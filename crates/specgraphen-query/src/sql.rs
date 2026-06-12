//! SQL-specific source conventions: `CREATE TABLE` DDL parsing and column
//! reference detection in SQL statements.
//!
//! Like [`crate::java`], this isolates language-specific heuristics from the
//! language-neutral query logic. Detection is line-based: a column reference
//! is found when it shares a line with the SQL keyword that gives it context,
//! which matches how SQL typically appears in `.sql` files and string
//! literals.

/// A column definition parsed from a `CREATE TABLE` statement.
#[derive(Debug)]
pub struct SqlColumnDef {
    pub column_name: String,
    pub sql_type: String,
    /// `COMMENT '...'` text, if present.
    pub comment: Option<String>,
}

/// A table definition parsed from a `CREATE TABLE` statement.
#[derive(Debug)]
pub struct SqlTableDef {
    pub table_name: String,
    pub columns: Vec<SqlColumnDef>,
}

/// How a SQL statement accesses a column.
#[derive(Debug, PartialEq, Eq)]
pub enum SqlAccess {
    Read,
    Write,
}

/// How a SQL statement accesses a table.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TableAccess {
    Create,
    Read,
    Update,
    Delete,
}

impl TableAccess {
    pub fn letter(self) -> char {
        match self {
            Self::Create => 'C',
            Self::Read => 'R',
            Self::Update => 'U',
            Self::Delete => 'D',
        }
    }
}

const CONSTRAINT_KEYWORDS: &[&str] = &[
    "PRIMARY",
    "FOREIGN",
    "UNIQUE",
    "CONSTRAINT",
    "KEY",
    "INDEX",
    "CHECK",
];

pub fn is_sql_file(path: &str) -> bool {
    path.to_lowercase().ends_with(".sql")
}

/// True if the line plausibly contains a SQL statement (used to find SQL
/// embedded in string literals of other languages).
pub fn looks_like_sql(line: &str) -> bool {
    let upper = line.to_uppercase();
    [
        "SELECT ",
        "INSERT INTO ",
        "UPDATE ",
        "DELETE FROM ",
        " FROM ",
        "WHERE ",
        " JOIN ",
    ]
    .iter()
    .any(|kw| upper.contains(kw))
}

/// Parse all `CREATE TABLE` statements in a SQL source.
pub fn parse_create_tables(source: &str) -> Vec<SqlTableDef> {
    let mut tables = Vec::new();
    let mut current: Option<SqlTableDef> = None;

    for line in source.lines() {
        let trimmed = line.trim();
        let upper = trimmed.to_uppercase();

        if let Some(table) = current.as_mut() {
            if trimmed.starts_with(')') {
                tables.push(current.take().expect("current table"));
                continue;
            }
            if let Some(col) = parse_column_line(trimmed) {
                table.columns.push(col);
            }
            if trimmed.ends_with(");") {
                tables.push(current.take().expect("current table"));
            }
        } else if upper.starts_with("CREATE TABLE") {
            let rest = trimmed["CREATE TABLE".len()..].trim_start();
            let rest = if rest.to_uppercase().starts_with("IF NOT EXISTS") {
                rest["IF NOT EXISTS".len()..].trim_start()
            } else {
                rest
            };
            let table_name = rest
                .split(|c: char| c.is_whitespace() || c == '(')
                .next()
                .unwrap_or_default()
                .trim_matches(['`', '"'])
                .to_string();
            if !table_name.is_empty() {
                current = Some(SqlTableDef {
                    table_name,
                    columns: Vec::new(),
                });
            }
        }
    }
    if let Some(table) = current.take() {
        tables.push(table);
    }

    tables
}

/// Classify how a SQL line accesses the given column, if it references it:
/// columns in an `INSERT INTO` list or between `UPDATE ... SET` and `WHERE`
/// are writes; references in `SELECT` / `WHERE` / `JOIN` context are reads.
pub fn column_access(line: &str, column_name: &str) -> Option<SqlAccess> {
    let upper = line.to_uppercase();
    let col_upper = column_name.to_uppercase();
    let pos = find_identifier(&upper, &col_upper)?;

    if let Some(insert_pos) = upper.find("INSERT INTO") {
        if pos > insert_pos {
            return Some(SqlAccess::Write);
        }
    }
    if let Some(set_pos) = find_identifier(&upper, "SET") {
        let where_pos = find_identifier(&upper, "WHERE").unwrap_or(usize::MAX);
        if pos > set_pos && pos < where_pos {
            return Some(SqlAccess::Write);
        }
    }
    if looks_like_sql(line) {
        return Some(SqlAccess::Read);
    }
    None
}

/// Classify how a SQL line accesses the given table, if it references it.
/// The table name is matched against the nearest preceding SQL keyword:
/// `INSERT INTO t` → Create, `FROM t` / `JOIN t` → Read, `UPDATE t` →
/// Update, `DELETE FROM t` → Delete.
pub fn table_access(line: &str, table_name: &str) -> Option<TableAccess> {
    let upper = line.to_uppercase();
    let table_upper = table_name.to_uppercase();
    let pos = find_identifier(&upper, &table_upper)?;

    const KEYWORDS: &[(&str, TableAccess)] = &[
        ("INSERT INTO", TableAccess::Create),
        ("DELETE FROM", TableAccess::Delete),
        ("UPDATE", TableAccess::Update),
        ("FROM", TableAccess::Read),
        ("JOIN", TableAccess::Read),
    ];

    let mut nearest: Option<(usize, TableAccess)> = None;
    for (kw, access) in KEYWORDS {
        let mut start = 0;
        while let Some(p) = upper[start..].find(kw) {
            let abs = start + p;
            // A bare FROM that is part of DELETE FROM is already covered
            let shadowed = *kw == "FROM" && upper[..abs].trim_end().ends_with("DELETE");
            if !shadowed && abs < pos && nearest.is_none_or(|(best, _)| abs > best) {
                nearest = Some((abs, *access));
            }
            start = abs + kw.len();
        }
    }
    nearest.map(|(_, access)| access)
}

fn parse_column_line(line: &str) -> Option<SqlColumnDef> {
    let mut tokens = line.split_whitespace();
    let name = tokens.next()?.trim_matches(['`', '"', ',']).to_string();
    if name.is_empty()
        || CONSTRAINT_KEYWORDS.contains(&name.to_uppercase().as_str())
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
    {
        return None;
    }
    let sql_type = tokens.next()?.trim_end_matches(',').to_string();
    let comment = quoted_after_keyword(line, "COMMENT");

    Some(SqlColumnDef {
        column_name: name,
        sql_type,
        comment,
    })
}

fn quoted_after_keyword(line: &str, keyword: &str) -> Option<String> {
    let pos = line.to_uppercase().find(keyword)?;
    line[pos..].split('\'').nth(1).map(str::to_string)
}

/// Boundary-checked, position-returning identifier search (so `id` does not
/// match inside `valid`). Both arguments must already share the same case.
fn find_identifier(haystack: &str, ident: &str) -> Option<usize> {
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(ident) {
        let abs = start + pos;
        let before_ok = abs == 0
            || haystack[..abs]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        let after = abs + ident.len();
        let after_ok = haystack[after..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        if before_ok && after_ok {
            return Some(abs);
        }
        start = abs + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_create_table_with_comments() {
        let ddl = r#"
CREATE TABLE user (
    id BIGINT NOT NULL COMMENT 'Primary key',
    email VARCHAR(255) NOT NULL COMMENT 'Mail address',
    created_at TIMESTAMP,
    PRIMARY KEY (id)
);
"#;
        let tables = parse_create_tables(ddl);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].table_name, "user");
        assert_eq!(tables[0].columns.len(), 3);
        assert_eq!(tables[0].columns[0].column_name, "id");
        assert_eq!(tables[0].columns[0].sql_type, "BIGINT");
        assert_eq!(tables[0].columns[0].comment.as_deref(), Some("Primary key"));
        assert_eq!(tables[0].columns[2].comment, None);
    }

    #[test]
    fn classifies_reads_and_writes() {
        let select = "SELECT id, email FROM user WHERE id = ?";
        assert_eq!(column_access(select, "email"), Some(SqlAccess::Read));
        assert_eq!(column_access(select, "id"), Some(SqlAccess::Read));

        let update = "UPDATE user SET email = ? WHERE id = ?";
        assert_eq!(column_access(update, "email"), Some(SqlAccess::Write));
        assert_eq!(column_access(update, "id"), Some(SqlAccess::Read));

        let insert = "INSERT INTO user (id, email) VALUES (?, ?)";
        assert_eq!(column_access(insert, "email"), Some(SqlAccess::Write));
    }

    #[test]
    fn classifies_table_access() {
        assert_eq!(
            table_access("SELECT * FROM user WHERE id = ?", "user"),
            Some(TableAccess::Read)
        );
        assert_eq!(
            table_access("INSERT INTO user (id) VALUES (?)", "user"),
            Some(TableAccess::Create)
        );
        assert_eq!(
            table_access("UPDATE user SET email = ?", "user"),
            Some(TableAccess::Update)
        );
        assert_eq!(
            table_access("DELETE FROM user WHERE id = ?", "user"),
            Some(TableAccess::Delete)
        );
        assert_eq!(table_access("SELECT * FROM orders", "user"), None);
    }

    #[test]
    fn respects_identifier_boundaries_and_non_sql_lines() {
        assert_eq!(
            column_access("SELECT valid FROM user", "id"),
            None,
            "id must not match inside valid"
        );
        assert_eq!(column_access("int id = compute();", "id"), None);
    }
}
