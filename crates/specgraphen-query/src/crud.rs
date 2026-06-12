use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use specgraphen_model::{JavaEntityType, SpaceData};

use crate::{java, mybatis, sql};

#[derive(Debug, Serialize, Deserialize)]
pub struct CrudMatrixResult {
    pub tables: Vec<TableCrud>,
    pub entry_points_analyzed: usize,
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TableCrud {
    pub table_class: String,
    pub table_name: String,
    pub entries: Vec<CrudEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CrudEntry {
    /// Entry-point method FQN (no callers in the analyzed sources).
    pub entry_point: String,
    /// Subset of "CRUD", in that order.
    pub operations: String,
    /// Methods or SQL statements where the operations were observed.
    pub evidence: Vec<String>,
}

/// table fqn -> entry fqn -> (CRUD ops, evidence)
type Matrix = BTreeMap<String, BTreeMap<String, (BTreeSet<char>, BTreeSet<String>)>>;

struct Table {
    class_fqn: String,
    simple: String,
    table_name: String,
}

pub fn crud_matrix(
    space_data: &SpaceData,
    source_files: &HashMap<String, String>,
) -> Result<CrudMatrixResult> {
    let fqn_of: HashMap<&str, &str> = space_data
        .fqn_to_cell_id
        .iter()
        .map(|(fqn, cell_id)| (cell_id.as_str(), fqn.as_str()))
        .collect();

    // Call-graph adjacency and incoming-call set, over resolved edges
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut has_callers: HashSet<&str> = HashSet::new();
    for inc in &space_data.incidences {
        if inc.relation_type == "java.calls" || inc.relation_type == "java.constructs" {
            if let (Some(from), Some(to)) = (
                fqn_of.get(inc.from_cell_id.as_str()),
                fqn_of.get(inc.to_cell_id.as_str()),
            ) {
                adjacency.entry(from).or_default().push(to);
                if inc.relation_type == "java.calls" {
                    has_callers.insert(to);
                }
            }
        }
    }

    let tables = find_table_classes(space_data, source_files);
    if tables.is_empty() {
        anyhow::bail!("No data classes found to build a CRUD matrix for");
    }

    // Repository-variable bindings per file: `UserRepository repository` → repository: User
    let mut repo_bindings: HashMap<&str, Vec<(String, String)>> = HashMap::new();
    for (path, content) in source_files {
        if path.ends_with(".java") {
            let bindings = repository_bindings(content);
            if !bindings.is_empty() {
                repo_bindings.insert(path, bindings);
            }
        }
    }

    // MyBatis mapper statements: Java FQN → statement
    let mut mapper_statements: HashMap<String, mybatis::MapperStatement> = HashMap::new();
    for (path, content) in source_files {
        if mybatis::is_mapper_xml(path, content) {
            for stmt in mybatis::parse_mapper_statements(content) {
                mapper_statements.insert(stmt.fqn.clone(), stmt);
            }
        }
    }

    // Entry points: non-accessor methods nobody calls
    let entry_points: Vec<&specgraphen_model::EntityRecord> = space_data
        .entities
        .iter()
        .filter(|e| {
            matches!(e.entity_type, JavaEntityType::Method)
                && !has_callers.contains(e.fqn.as_str())
                && !java::is_accessor(&e.label)
        })
        .collect();

    let entity_by_fqn: HashMap<&str, &specgraphen_model::EntityRecord> = space_data
        .entities
        .iter()
        .map(|e| (e.fqn.as_str(), e))
        .collect();

    // table fqn → entry fqn → (ops, evidence)
    let mut matrix = Matrix::new();

    for entry in &entry_points {
        for reached_fqn in reachable_from(&entry.fqn, &adjacency) {
            // SQL inside the body of a reached Java method
            if let Some(method) = entity_by_fqn.get(reached_fqn.as_str()) {
                for line in body_lines(method, source_files) {
                    for table in &tables {
                        if let Some(access) = sql::table_access(line, &table.table_name) {
                            record(
                                &mut matrix,
                                &table.class_fqn,
                                &entry.fqn,
                                access.letter(),
                                reached_fqn.clone(),
                            );
                        }
                    }
                }
                // Repository-call conventions in the same body
                if let Some(bindings) = repo_bindings.get(method.witness.file.as_str()) {
                    for line in body_lines(method, source_files) {
                        for (var, entity_simple) in bindings {
                            let Some(called) = called_method_on(line, var) else {
                                continue;
                            };
                            let Some(ops) = java::repository_operation(&called) else {
                                continue;
                            };
                            for table in tables.iter().filter(|t| &t.simple == entity_simple) {
                                for op in ops.chars() {
                                    record(
                                        &mut matrix,
                                        &table.class_fqn,
                                        &entry.fqn,
                                        op,
                                        format!("{reached_fqn} → .{called}()"),
                                    );
                                }
                            }
                        }
                    }
                }
            }
            // SQL in a MyBatis mapper statement backing a reached interface method
            if let Some(stmt) = mapper_statements.get(&reached_fqn) {
                for line in stmt.sql.lines() {
                    for table in &tables {
                        if let Some(access) = sql::table_access(line, &table.table_name) {
                            record(
                                &mut matrix,
                                &table.class_fqn,
                                &entry.fqn,
                                access.letter(),
                                format!("{} (mapper XML)", stmt.fqn),
                            );
                        }
                    }
                }
            }
        }
    }

    let result_tables: Vec<TableCrud> = tables
        .iter()
        .filter_map(|table| {
            let entries = matrix.get(table.class_fqn.as_str())?;
            Some(TableCrud {
                table_class: table.class_fqn.clone(),
                table_name: table.table_name.clone(),
                entries: entries
                    .iter()
                    .map(|(entry_point, (ops, evidence))| CrudEntry {
                        entry_point: entry_point.to_string(),
                        operations: "CRUD".chars().filter(|c| ops.contains(c)).collect(),
                        evidence: evidence.iter().take(5).cloned().collect(),
                    })
                    .collect(),
            })
        })
        .collect();

    Ok(CrudMatrixResult {
        tables: result_tables,
        entry_points_analyzed: entry_points.len(),
        note: "Operations are derived from SQL statements (in Java strings, .sql files, and \
               MyBatis mapper XML) and repository naming conventions, reached from each entry \
               point over the resolved call graph. Unresolved calls may hide operations — check \
               the unknowns tool for gaps."
            .to_string(),
    })
}

fn record(matrix: &mut Matrix, table_fqn: &str, entry_fqn: &str, op: char, evidence: String) {
    let slot = matrix
        .entry(table_fqn.to_string())
        .or_default()
        .entry(entry_fqn.to_string())
        .or_default();
    slot.0.insert(op);
    slot.1.insert(evidence);
}

/// Data classes: classes with fields whose other members are only
/// constructors and accessors (or that match a DDL table in the sources).
fn find_table_classes(
    space_data: &SpaceData,
    source_files: &HashMap<String, String>,
) -> Vec<Table> {
    let mut tables = Vec::new();

    for entity in &space_data.entities {
        if !matches!(entity.entity_type, JavaEntityType::Class) {
            continue;
        }
        let prefix = format!("{}.", entity.fqn);
        let members: Vec<_> = space_data
            .entities
            .iter()
            .filter(|e| e.fqn.starts_with(&prefix))
            .collect();
        let has_fields = members
            .iter()
            .any(|e| matches!(e.entity_type, JavaEntityType::Field));
        let all_plain = members.iter().all(|e| match e.entity_type {
            JavaEntityType::Field | JavaEntityType::Constructor => true,
            JavaEntityType::Method => java::is_accessor(&e.label),
            _ => true,
        });
        if !has_fields || !all_plain {
            continue;
        }

        let simple = entity.fqn.rsplit('.').next().unwrap_or(&entity.fqn);
        let table_name = source_files
            .get(&entity.witness.file)
            .and_then(|src| java::table_annotation_name(src))
            .unwrap_or_else(|| java::default_sql_name(simple));

        tables.push(Table {
            class_fqn: entity.fqn.clone(),
            simple: simple.to_string(),
            table_name,
        });
    }

    tables
}

fn reachable_from(entry_fqn: &str, adjacency: &HashMap<&str, Vec<&str>>) -> Vec<String> {
    let mut visited: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    visited.insert(entry_fqn);
    queue.push_back(entry_fqn);
    while let Some(current) = queue.pop_front() {
        if let Some(nexts) = adjacency.get(current) {
            for next in nexts {
                if visited.insert(next) {
                    queue.push_back(next);
                }
            }
        }
    }
    visited.into_iter().map(String::from).collect()
}

fn body_lines<'a>(
    entity: &specgraphen_model::EntityRecord,
    source_files: &'a HashMap<String, String>,
) -> Vec<&'a str> {
    let Some(content) = source_files.get(&entity.witness.file) else {
        return Vec::new();
    };
    let start = entity.witness.start_line as usize;
    let end = entity.witness.end_line as usize;
    if start == 0 || end < start {
        return Vec::new();
    }
    content
        .lines()
        .skip(start - 1)
        .take(end - start + 1)
        .collect()
}

/// Variables of repository-ish types declared in a Java source:
/// `UserRepository repository` → ("repository", "User").
fn repository_bindings(content: &str) -> Vec<(String, String)> {
    let mut bindings = Vec::new();
    for line in content.lines() {
        let tokens: Vec<&str> = line
            .split(|c: char| c.is_whitespace() || ['(', ')', ',', ';'].contains(&c))
            .filter(|t| !t.is_empty())
            .collect();
        for window in tokens.windows(2) {
            let [type_token, var_token] = window else {
                continue;
            };
            let Some(entity_simple) = java::repository_entity_name(type_token) else {
                continue;
            };
            if var_token.chars().all(|c| c.is_alphanumeric() || c == '_')
                && var_token.chars().next().is_some_and(char::is_lowercase)
            {
                bindings.push((var_token.to_string(), entity_simple.to_string()));
            }
        }
    }
    bindings.sort();
    bindings.dedup();
    bindings
}

/// Method name called on `var` in this line (`repository.save(user)` with
/// var "repository" → "save").
fn called_method_on(line: &str, var: &str) -> Option<String> {
    let pat = format!("{var}.");
    let mut start = 0;
    while let Some(pos) = line[start..].find(&pat) {
        let abs = start + pos;
        let before_ok = abs == 0
            || line[..abs]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        if before_ok {
            let rest = &line[abs + pat.len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() && rest[name.len()..].starts_with('(') {
                return Some(name);
            }
        }
        start = abs + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_repository_bindings_and_calls() {
        let source = "public class S { private final UserRepository repository;\n\
                      public S(OrderDao orderDao) {} }";
        let bindings = repository_bindings(source);
        assert!(bindings.contains(&("repository".to_string(), "User".to_string())));
        assert!(bindings.contains(&("orderDao".to_string(), "Order".to_string())));

        assert_eq!(
            called_method_on("return repository.save(user);", "repository"),
            Some("save".to_string())
        );
        assert_eq!(called_method_on("myrepository.save(x)", "repository"), None);
    }
}
