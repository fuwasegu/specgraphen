//! Java-specific source conventions: field declarations, JPA annotations,
//! and JavaBeans accessor naming.
//!
//! Language-specific heuristics live here so that query logic stays
//! language-neutral. Supporting another language means adding a sibling
//! module with the same surface.

/// A field declaration parsed from Java source.
#[derive(Debug)]
pub struct FieldDecl {
    pub field_name: String,
    pub declared_type: String,
    /// Column name from a JPA `@Column(name = "...")` annotation, if present.
    pub column_name: Option<String>,
    /// Documentation attached to the field: javadoc block, preceding line
    /// comment, or inline comment on the declaration line.
    pub doc: Option<String>,
}

const MODIFIERS: &[&str] = &[
    "public",
    "private",
    "protected",
    "static",
    "final",
    "transient",
    "volatile",
];

const STATEMENT_KEYWORDS: &[&str] = &[
    "return", "throw", "if", "for", "while", "switch", "case", "break", "continue", "package",
    "import", "new", "else", "do", "try", "catch", "assert", "yield",
];

/// Parse all field declarations from a Java source file, attaching JPA
/// `@Column` names and documentation comments to each field.
pub fn parse_field_declarations(source: &str) -> Vec<FieldDecl> {
    let mut decls = Vec::new();
    let mut pending_doc: Option<String> = None;
    let mut pending_column: Option<String> = None;
    let mut in_javadoc = false;

    for line in source.lines() {
        let trimmed = line.trim();

        if in_javadoc {
            if pending_doc.is_none() {
                pending_doc = javadoc_text(trimmed);
            }
            if trimmed.ends_with("*/") {
                in_javadoc = false;
            }
            continue;
        }
        if trimmed.starts_with("/**") {
            pending_doc = javadoc_text(trimmed);
            in_javadoc = !trimmed.ends_with("*/");
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }

        let (code, inline_comment) = split_inline_comment(trimmed);

        if code.is_empty() {
            if inline_comment.is_some() {
                pending_doc = inline_comment;
            }
            continue;
        }
        if code.starts_with('@') {
            if code.starts_with("@Column") {
                pending_column = quoted_arg(code, "name");
            }
            continue;
        }

        if let Some((field_name, declared_type)) = parse_declaration(code) {
            decls.push(FieldDecl {
                field_name,
                declared_type,
                column_name: pending_column.take(),
                doc: inline_comment.or_else(|| pending_doc.take()),
            });
        }
        pending_doc = None;
        pending_column = None;
    }

    decls
}

/// JavaBeans getter call patterns for a field, e.g. `.getEmail(` / `.isActive(`.
pub fn getter_patterns(field_name: &str) -> Vec<String> {
    let cap = capitalize(field_name);
    vec![format!(".get{cap}("), format!(".is{cap}(")]
}

/// JavaBeans setter call pattern for a field, e.g. `.setEmail(`.
pub fn setter_pattern(field_name: &str) -> String {
    format!(".set{}(", capitalize(field_name))
}

/// True if `line` contains a direct access `.field` (with an identifier
/// boundary after it, so `id` does not match `.identifier`).
pub fn has_direct_field_access(line: &str, field_name: &str) -> bool {
    direct_access_position(line, field_name).is_some()
}

/// True if the line assigns to `.field` (`x.field = ...`, but not `==`).
pub fn is_direct_field_write(line: &str, field_name: &str) -> bool {
    if let Some(pos) = direct_access_position(line, field_name) {
        let rest = line[pos + field_name.len() + 1..].trim_start();
        return rest.starts_with('=') && !rest.starts_with("==");
    }
    false
}

fn direct_access_position(line: &str, field_name: &str) -> Option<usize> {
    let pat = format!(".{field_name}");
    let mut start = 0;
    while let Some(pos) = line[start..].find(&pat) {
        let abs = start + pos;
        let after = abs + pat.len();
        let boundary = line[after..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        if boundary {
            return Some(abs);
        }
        start = abs + 1;
    }
    None
}

fn parse_declaration(code: &str) -> Option<(String, String)> {
    if !code.ends_with(';') {
        return None;
    }
    // Only the part before any initializer is the declaration.
    let decl = code
        .split('=')
        .next()
        .unwrap_or(code)
        .trim()
        .trim_end_matches(';')
        .trim();
    if decl.contains('(') || decl.contains(')') || decl.contains('{') {
        return None;
    }
    let mut tokens: Vec<&str> = decl.split_whitespace().collect();
    if let Some(first) = tokens.first() {
        if STATEMENT_KEYWORDS.contains(first) {
            return None;
        }
    }
    tokens.retain(|t| !MODIFIERS.contains(t));
    if tokens.len() < 2 {
        return None;
    }
    let field_name = tokens.pop()?.to_string();
    if field_name.is_empty()
        || !field_name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
    {
        return None;
    }
    Some((field_name, tokens.join(" ")))
}

fn split_inline_comment(line: &str) -> (&str, Option<String>) {
    match line.split_once("//") {
        Some((code, comment)) => {
            let comment = comment.trim();
            (
                code.trim_end(),
                (!comment.is_empty()).then(|| comment.to_string()),
            )
        }
        None => (line, None),
    }
}

fn quoted_arg(line: &str, key: &str) -> Option<String> {
    let after = &line[line.find(key)? + key.len()..];
    after.split('"').nth(1).map(|s| s.to_string())
}

fn javadoc_text(line: &str) -> Option<String> {
    let text = line
        .trim_start_matches("/**")
        .trim_start_matches('*')
        .trim_end_matches("*/")
        .trim();
    if text.is_empty() || text.starts_with('@') {
        None
    } else {
        Some(text.to_string())
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_field_declarations() {
        let source = r#"
public class User {
    private Long id;
    private String name = "anonymous";
    private java.util.Map<String, Integer> scores;

    public String getName() {
        return name;
    }
}
"#;
        let decls = parse_field_declarations(source);
        assert_eq!(decls.len(), 3);
        assert_eq!(decls[0].field_name, "id");
        assert_eq!(decls[0].declared_type, "Long");
        assert_eq!(decls[1].field_name, "name");
        assert_eq!(decls[1].declared_type, "String");
        assert_eq!(decls[2].field_name, "scores");
        assert_eq!(decls[2].declared_type, "java.util.Map<String, Integer>");
    }

    #[test]
    fn attaches_jpa_column_and_javadoc() {
        let source = r#"
public class Customer {
    /** Customer mail address */
    @Column(name = "mail_address", length = 255)
    private String email;

    private boolean active; // soft-delete flag
}
"#;
        let decls = parse_field_declarations(source);
        assert_eq!(decls.len(), 2);
        assert_eq!(decls[0].column_name.as_deref(), Some("mail_address"));
        assert_eq!(decls[0].doc.as_deref(), Some("Customer mail address"));
        assert_eq!(decls[1].column_name, None);
        assert_eq!(decls[1].doc.as_deref(), Some("soft-delete flag"));
    }

    #[test]
    fn field_access_respects_identifier_boundary() {
        assert!(has_direct_field_access("user.id == 1", "id"));
        assert!(!has_direct_field_access("user.identifier == 1", "id"));
        assert!(is_direct_field_write("user.id = 1;", "id"));
        assert!(!is_direct_field_write("user.id == 1", "id"));
    }
}
