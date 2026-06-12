use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use specgraphen_model::SpaceData;

#[derive(Debug, Serialize, Deserialize)]
pub struct ColumnUsageResult {
    pub table_class: String,
    pub table_description: Option<String>,
    pub columns: Vec<ColumnInfo>,
    pub total_columns: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub field_name: String,
    pub logical_name: String,
    pub column_name: String,
    pub data_type: String,
    pub readers: Vec<UsageSite>,
    pub writers: Vec<UsageSite>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UsageSite {
    pub fqn: String,
    pub file: String,
    pub line: u32,
    pub access_type: String,
}

pub fn column_usage(
    space_data: &SpaceData,
    table_class: &str,
    source_files: &HashMap<String, String>,
) -> Result<ColumnUsageResult> {
    let table_lower = table_class.to_lowercase();

    // Find the data class
    let class_entity = space_data
        .entities
        .iter()
        .find(|e| {
            matches!(e.entity_type, specgraphen_model::JavaEntityType::Class)
                && (e.fqn.to_lowercase().ends_with(&format!(".{table_lower}"))
                    || e.fqn.to_lowercase() == table_lower
                    || e.label.to_lowercase() == table_lower)
        })
        .ok_or_else(|| anyhow::anyhow!("Table class not found: {table_class}"))?;

    let class_fqn = &class_entity.fqn;

    // Parse source to extract makeAttribute patterns
    let source = source_files
        .get(&class_entity.witness.file)
        .cloned()
        .unwrap_or_default();

    let column_defs = parse_make_attribute_calls(&source);

    // Find all field entities for this class
    let fields: Vec<_> = space_data
        .entities
        .iter()
        .filter(|e| {
            e.fqn.starts_with(class_fqn)
                && e.fqn != *class_fqn
                && matches!(e.entity_type, specgraphen_model::JavaEntityType::Field)
        })
        .collect();

    // For each field, search all source files for usage
    let mut columns = Vec::new();
    for field in &fields {
        let field_name = field.fqn.rsplit('.').next().unwrap_or(&field.fqn);

        let col_def = column_defs.iter().find(|d| d.field_name == field_name);

        let logical_name = col_def.map(|d| d.logical_name.clone()).unwrap_or_default();
        let column_name = col_def
            .map(|d| d.column_name.clone())
            .unwrap_or_else(|| field_name.to_string());
        let data_type = col_def
            .map(|d| d.data_type.clone())
            .unwrap_or_else(|| "unknown".to_string());

        let (readers, writers) = find_field_usages(field_name, class_fqn, source_files);

        if !readers.is_empty() || !writers.is_empty() || col_def.is_some() {
            columns.push(ColumnInfo {
                field_name: field_name.to_string(),
                logical_name,
                column_name,
                data_type,
                readers,
                writers,
            });
        }
    }

    columns.sort_by(|a, b| {
        let a_usage = a.readers.len() + a.writers.len();
        let b_usage = b.readers.len() + b.writers.len();
        b_usage.cmp(&a_usage)
    });

    // Extract table description from javadoc
    let table_description = extract_javadoc(&source);

    let total_columns = columns.len();

    Ok(ColumnUsageResult {
        table_class: class_fqn.clone(),
        table_description,
        columns,
        total_columns,
    })
}

#[derive(Debug)]
struct ColumnDef {
    field_name: String,
    logical_name: String,
    column_name: String,
    data_type: String,
}

fn parse_make_attribute_calls(source: &str) -> Vec<ColumnDef> {
    let mut defs = Vec::new();

    for line in source.lines() {
        let trimmed = line.trim();

        // Pattern: public DataAttributeXxx FIELD_NAME = makeAttribute("論理名", "カラム名", new DataDomainXxx(...));
        if let Some(pos) = trimmed.find("makeAttribute(") {
            // Extract field name
            let field_name = trimmed
                .split_whitespace()
                .take_while(|w| !w.contains("makeAttribute"))
                .filter(|w| {
                    !w.starts_with("public")
                        && !w.starts_with("private")
                        && !w.starts_with("protected")
                        && !w.starts_with("DataAttribute")
                        && *w != "="
                        && !w.is_empty()
                })
                .last()
                .unwrap_or("")
                .to_string();

            // Extract arguments from makeAttribute("logical", "column", ...)
            let args_start = pos + "makeAttribute(".len();
            let args_str = &trimmed[args_start..];

            let parts: Vec<&str> = args_str.split('"').collect();
            let logical_name = parts.get(1).unwrap_or(&"").to_string();
            let column_name = parts.get(3).unwrap_or(&"").to_string();

            // Extract data type from DataDomainXxx or DataAttributeXxx
            let data_type = if trimmed.contains("DataDomainString")
                || trimmed.contains("DataAttributeString")
            {
                "String"
            } else if trimmed.contains("DataDomainNumeric")
                || trimmed.contains("DataAttributeNumeric")
            {
                "Numeric"
            } else if trimmed.contains("DataDomainDateTime")
                || trimmed.contains("DataAttributeDateTime")
            {
                "DateTime"
            } else if trimmed.contains("DataDomainBlob") || trimmed.contains("DataAttributeBlob") {
                "Blob"
            } else if trimmed.contains("NumericFlag") {
                "Flag"
            } else {
                "unknown"
            }
            .to_string();

            if !field_name.is_empty() {
                defs.push(ColumnDef {
                    field_name,
                    logical_name,
                    column_name,
                    data_type,
                });
            }
        }
    }

    defs
}

fn find_field_usages(
    field_name: &str,
    class_fqn: &str,
    source_files: &HashMap<String, String>,
) -> (Vec<UsageSite>, Vec<UsageSite>) {
    let mut readers = Vec::new();
    let mut writers = Vec::new();

    let class_simple = class_fqn.rsplit('.').next().unwrap_or(class_fqn);
    let class_file_hint = format!("{class_simple}.java");

    for (file_path, content) in source_files {
        // Skip the class's own file for self-references
        if file_path.ends_with(&class_file_hint) {
            continue;
        }

        for (line_num, line) in content.lines().enumerate() {
            let line_num = line_num as u32 + 1;

            // Look for field access: .FIELD_NAME with word boundary
            if !line.contains(field_name) {
                continue;
            }

            let access_pattern = format!(".{field_name}");
            if !line.contains(&access_pattern) {
                continue;
            }

            // Determine read vs write
            let trimmed = line.trim();
            let is_write = trimmed.contains(&format!(".{field_name}.setValue"))
                || trimmed.contains(&format!(".{field_name}.set("))
                || trimmed.contains(&format!(".{field_name} ="))
                || trimmed.contains(&format!(".{field_name}="));

            let is_read = trimmed.contains(&format!(".{field_name}.getValue"))
                || trimmed.contains(&format!(".{field_name}.get("))
                || trimmed.contains(&format!(".{field_name}.toString"))
                || (trimmed.contains(&access_pattern) && !is_write);

            let inferred_class = infer_class_from_file(file_path);

            let site = UsageSite {
                fqn: inferred_class,
                file: file_path.clone(),
                line: line_num,
                access_type: if is_write && is_read {
                    "read_write".to_string()
                } else if is_write {
                    "write".to_string()
                } else {
                    "read".to_string()
                },
            };

            if is_write {
                writers.push(site);
            } else if is_read {
                readers.push(site);
            }
        }
    }

    readers.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
    writers.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));

    readers.dedup_by(|a, b| a.file == b.file && a.line == b.line);
    writers.dedup_by(|a, b| a.file == b.file && a.line == b.line);

    (readers, writers)
}

fn infer_class_from_file(file_path: &str) -> String {
    file_path
        .trim_end_matches(".java")
        .replace(['/', '\\'], ".")
}

fn extract_javadoc(source: &str) -> Option<String> {
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("* ") && !trimmed.starts_with("* @") && trimmed.len() > 2 {
            let desc = trimmed.trim_start_matches("* ").trim();
            if !desc.is_empty() {
                return Some(desc.to_string());
            }
        }
    }
    None
}
