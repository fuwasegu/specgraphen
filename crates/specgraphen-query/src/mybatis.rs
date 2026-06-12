//! MyBatis-specific source conventions: mapper XML parsing.
//!
//! Legacy Java codebases often wire Java interfaces to SQL through MyBatis
//! mapper XML files. Lifting those statements lets the call graph and CRUD
//! analysis see through that indirection: `<mapper namespace="x.y.UserMapper">`
//! with `<select id="findById">` corresponds to the Java method
//! `x.y.UserMapper.findById`.

/// One SQL statement in a mapper XML file.
#[derive(Debug)]
pub struct MapperStatement {
    /// Java-side FQN this statement implements: `namespace.id`.
    pub fqn: String,
    pub sql: String,
}

const STATEMENT_TAGS: &[&str] = &["select", "insert", "update", "delete"];

pub fn is_mapper_xml(path: &str, content: &str) -> bool {
    path.to_lowercase().ends_with(".xml") && content.contains("<mapper")
}

/// Parse all SQL statements from a MyBatis mapper XML source.
pub fn parse_mapper_statements(content: &str) -> Vec<MapperStatement> {
    let mut statements = Vec::new();
    let mut namespace: Option<String> = None;
    let mut open: Option<(String, String, Vec<String>)> = None; // (kind, id, sql lines)

    for line in content.lines() {
        let trimmed = line.trim();

        if namespace.is_none() {
            if let Some(ns) = attr_on_tag(trimmed, "mapper", "namespace") {
                namespace = Some(ns);
            }
        }

        if let Some((kind, id, mut sql_lines)) = open.take() {
            let close_tag = format!("</{kind}>");
            if let Some(pos) = trimmed.find(&close_tag) {
                let before = trimmed[..pos].trim();
                if !before.is_empty() {
                    sql_lines.push(before.to_string());
                }
                if let Some(ref ns) = namespace {
                    statements.push(MapperStatement {
                        fqn: format!("{ns}.{id}"),
                        sql: sql_lines.join("\n"),
                    });
                }
            } else {
                if !trimmed.is_empty() && !trimmed.starts_with("<!--") {
                    sql_lines.push(trimmed.to_string());
                }
                open = Some((kind, id, sql_lines));
            }
            continue;
        }

        for tag in STATEMENT_TAGS {
            if !trimmed.starts_with(&format!("<{tag}")) {
                continue;
            }
            let Some(id) = attr_on_tag(trimmed, tag, "id") else {
                continue;
            };
            // Single-line statement: <select id="x">SQL</select>
            let close_tag = format!("</{tag}>");
            if let (Some(gt), Some(close)) = (trimmed.find('>'), trimmed.find(&close_tag)) {
                if gt < close {
                    let sql = trimmed[gt + 1..close].trim().to_string();
                    if let Some(ref ns) = namespace {
                        statements.push(MapperStatement {
                            fqn: format!("{ns}.{id}"),
                            sql,
                        });
                    }
                    break;
                }
            }
            open = Some((tag.to_string(), id, Vec::new()));
            break;
        }
    }

    statements
}

fn attr_on_tag(line: &str, tag: &str, attr: &str) -> Option<String> {
    if !line.starts_with(&format!("<{tag}")) {
        return None;
    }
    let pos = line.find(&format!("{attr}="))?;
    line[pos..].split('"').nth(1).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mapper_statements() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<mapper namespace="com.example.mapper.UserMapper">
    <select id="findById" resultType="User">
        SELECT id, name, email FROM user WHERE id = #{id}
    </select>
    <update id="updateEmail">UPDATE user SET email = #{email} WHERE id = #{id}</update>
</mapper>
"#;
        let statements = parse_mapper_statements(xml);
        assert_eq!(statements.len(), 2);
        assert_eq!(statements[0].fqn, "com.example.mapper.UserMapper.findById");
        assert!(statements[0].sql.contains("FROM user"));
        assert_eq!(
            statements[1].fqn,
            "com.example.mapper.UserMapper.updateEmail"
        );
        assert!(statements[1].sql.contains("SET email"));
    }

    #[test]
    fn detects_mapper_files() {
        assert!(is_mapper_xml("UserMapper.xml", "<mapper namespace=\"x\">"));
        assert!(!is_mapper_xml("pom.xml", "<project></project>"));
        assert!(!is_mapper_xml("User.java", "<mapper"));
    }
}
