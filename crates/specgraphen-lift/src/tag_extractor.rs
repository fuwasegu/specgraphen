//! Extracts decision tables from JSP / tag-file conditional markup.
//!
//! Legacy JSP screens hold their display logic in JSTL conditionals:
//! `<c:if test="${...}">` and `<c:choose>/<c:when>/<c:otherwise>`. This
//! extractor walks that structure the same way the Java extractor walks
//! `if`/`switch`: EL test expressions are atomized by normalized text,
//! `c:when` chains get sequential else-if semantics, and the outcome of a
//! path is the content it renders (text summaries of the conditional
//! bodies, with content common to every path subtracted).
//!
//! JSP is not well-formed XML, so this is a tolerant scanner that tracks
//! only `c:`-namespace conditional tags and treats everything else as
//! opaque content. Scriptlet conditionals (`<% if ... %>`) cannot be
//! modeled and mark the extraction [`TagDecision::incomplete`].

use specgraphen_logic::{DecisionTable, Tri};

const MAX_ATOMS: usize = specgraphen_logic::MAX_VARIABLES;
const MAX_PATHS: usize = 512;
/// Per-text-chunk cap in outcome labels.
const SUMMARY_CHARS: usize = 60;

/// Decision table for one top-level conditional cluster in a JSP/tag file.
#[derive(Debug)]
pub struct TagDecision {
    /// 1-based line of the cluster's opening tag.
    pub start_line: u32,
    pub table: DecisionTable,
    /// Scriptlet conditionals (`<% if`) exist in the file; the table may
    /// miss display logic expressed there.
    pub incomplete: bool,
}

#[derive(Debug, Default)]
pub struct TagExtraction {
    pub clusters: Vec<TagDecision>,
    /// Clusters that could not be extracted: (location, reason).
    pub skipped: Vec<(String, String)>,
}

#[derive(Debug)]
enum Node {
    If {
        test: String,
        body: Vec<Node>,
    },
    Choose {
        whens: Vec<(String, Vec<Node>)>,
        otherwise: Option<Vec<Node>>,
    },
    Text(String),
}

pub struct TagExtractor;

impl TagExtractor {
    pub fn new() -> Self {
        Self
    }

    pub fn extract(&self, source: &str) -> TagExtraction {
        let mut result = TagExtraction::default();
        let scriptlet_offsets = scriptlet_conditional_offsets(source);

        let clusters = match parse_clusters(source) {
            Ok(c) => c,
            Err(reason) => {
                result.skipped.push(("(file)".to_string(), reason));
                return result;
            }
        };

        for (line, span, nodes) in clusters {
            // Only a scriptlet conditional INSIDE this cluster makes its own
            // table unfaithful; scriptlets elsewhere are other regions.
            let incomplete = scriptlet_offsets.iter().any(|&o| span.contains(&o));
            match build_table(&nodes) {
                Ok(Some(table)) => result.clusters.push(TagDecision {
                    start_line: line,
                    table,
                    incomplete,
                }),
                Ok(None) => {} // no conditions — nothing to compress
                Err(reason) => result.skipped.push((format!("line {line}"), reason)),
            }
        }
        result
    }
}

impl Default for TagExtractor {
    fn default() -> Self {
        Self::new()
    }
}

fn scriptlet_conditional_offsets(source: &str) -> Vec<usize> {
    source
        .match_indices("<%")
        .filter(|(i, _)| {
            let rest = source[i + 2..].trim_start();
            rest.starts_with("if") || rest.starts_with("} else") || rest.starts_with("else")
        })
        .map(|(i, _)| i)
        .collect()
}

/// (start line, byte span, nodes) of one top-level conditional cluster.
type Cluster = (u32, std::ops::Range<usize>, Vec<Node>);
/// One enumerated render path: condition assignments + rendered summaries.
type RenderPath = (Vec<(usize, bool)>, Vec<String>);

/// One scanned conditional tag occurrence.
#[derive(Debug, PartialEq)]
enum Tag {
    OpenIf(String),
    CloseIf,
    OpenChoose,
    CloseChoose,
    OpenWhen(String),
    CloseWhen,
    OpenOtherwise,
    CloseOtherwise,
}

/// Parse top-level conditional clusters: each top-level `c:if`/`c:choose`
/// becomes its own cluster (with everything nested inside it). Returns
/// (start line, byte span, nodes) per cluster.
fn parse_clusters(source: &str) -> Result<Vec<Cluster>, String> {
    let mut clusters = Vec::new();
    let mut pos = 0usize;

    while let Some((tag, tag_start, tag_end)) = next_tag(source, pos)? {
        match tag {
            Tag::OpenIf(_) | Tag::OpenChoose => {
                let line = line_of(source, tag_start);
                let (node, after) = parse_node(source, tag, tag_end)?;
                clusters.push((line, tag_start..after, vec![node]));
                pos = after;
            }
            // A closer at top level means malformed nesting
            _ => return Err(format!("unexpected closing tag at byte {tag_start}")),
        }
    }
    Ok(clusters)
}

/// Parse the node whose opening `tag` was just consumed; returns the node
/// and the byte offset just past its closing tag.
fn parse_node(source: &str, tag: Tag, mut pos: usize) -> Result<(Node, usize), String> {
    match tag {
        Tag::OpenIf(test) => {
            let (body, after) = parse_body(source, pos, &Tag::CloseIf)?;
            Ok((Node::If { test, body }, after))
        }
        Tag::OpenChoose => {
            let mut whens = Vec::new();
            let mut otherwise = None;
            loop {
                let Some((inner, _, inner_end)) = next_tag(source, pos)? else {
                    return Err("unclosed <c:choose>".to_string());
                };
                match inner {
                    Tag::OpenWhen(test) => {
                        let (body, after) = parse_body(source, inner_end, &Tag::CloseWhen)?;
                        whens.push((test, body));
                        pos = after;
                    }
                    Tag::OpenOtherwise => {
                        let (body, after) = parse_body(source, inner_end, &Tag::CloseOtherwise)?;
                        otherwise = Some(body);
                        pos = after;
                    }
                    Tag::CloseChoose => {
                        return Ok((Node::Choose { whens, otherwise }, inner_end));
                    }
                    other => return Err(format!("unexpected {other:?} inside <c:choose>")),
                }
            }
        }
        other => Err(format!("internal: parse_node on {other:?}")),
    }
}

/// Parse children until the expected closing tag; text between tags becomes
/// content nodes.
fn parse_body(source: &str, mut pos: usize, until: &Tag) -> Result<(Vec<Node>, usize), String> {
    let mut nodes = Vec::new();
    loop {
        let Some((tag, tag_start, tag_end)) = next_tag(source, pos)? else {
            return Err(format!("missing {until:?}"));
        };
        if let Some(text) = summarize(&source[pos..tag_start]) {
            nodes.push(Node::Text(text));
        }
        if &tag == until {
            return Ok((nodes, tag_end));
        }
        match tag {
            Tag::OpenIf(_) | Tag::OpenChoose => {
                let (node, after) = parse_node(source, tag, tag_end)?;
                nodes.push(node);
                pos = after;
            }
            other => return Err(format!("unexpected {other:?} (expected {until:?})")),
        }
    }
}

/// Find the next `c:` conditional tag at or after `pos`.
/// Returns (tag, start offset, offset just past `>`).
fn next_tag(source: &str, mut pos: usize) -> Result<Option<(Tag, usize, usize)>, String> {
    let bytes = source.as_bytes();
    while let Some(found) = source[pos..]
        .find("<c:")
        .map(|i| pos + i)
        .or_else(|| source[pos..].find("</c:").map(|i| pos + i))
    {
        // `find` above prefers "<c:"; check both candidates and take the earliest
        let open = source[pos..].find("<c:").map(|i| pos + i);
        let close = source[pos..].find("</c:").map(|i| pos + i);
        let start = match (open, close) {
            (Some(o), Some(c)) => o.min(c),
            (Some(o), None) => o,
            (None, Some(c)) => c,
            (None, None) => unreachable!(),
        };
        let _ = found;

        let closing = bytes.get(start + 1) == Some(&b'/');
        let name_start = if closing { start + 2 } else { start + 1 };
        let rest = &source[name_start..];
        let kind = ["c:if", "c:choose", "c:when", "c:otherwise"]
            .iter()
            .find(|k| {
                rest.starts_with(**k)
                    && !rest[k.len()..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_alphanumeric())
            })
            .copied();

        let Some(kind) = kind else {
            // some other c:-tag (c:forEach, c:set, …): treat as content
            pos = start + 1;
            continue;
        };

        let tag_body_start = name_start + kind.len();
        let end = find_tag_end(source, tag_body_start)
            .ok_or_else(|| format!("unterminated tag at byte {start}"))?;
        let attrs = &source[tag_body_start..end - 1];
        let self_closing = attrs.trim_end().ends_with('/');

        let tag = match (kind, closing) {
            ("c:if", false) => {
                if self_closing {
                    pos = end; // bodyless <c:if/> renders nothing — skip
                    continue;
                }
                Tag::OpenIf(extract_test(attrs)?)
            }
            ("c:if", true) => Tag::CloseIf,
            ("c:choose", false) => Tag::OpenChoose,
            ("c:choose", true) => Tag::CloseChoose,
            ("c:when", false) => Tag::OpenWhen(extract_test(attrs)?),
            ("c:when", true) => Tag::CloseWhen,
            ("c:otherwise", false) => Tag::OpenOtherwise,
            ("c:otherwise", true) => Tag::CloseOtherwise,
            _ => unreachable!(),
        };
        return Ok(Some((tag, start, end)));
    }
    Ok(None)
}

/// Offset just past the `>` that ends a tag opened before `pos`, respecting
/// quoted attribute values (EL can contain `>`).
fn find_tag_end(source: &str, pos: usize) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (i, c) in source[pos..].char_indices() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"') | (None, '\'') => quote = Some(c),
            (None, '>') => return Some(pos + i + 1),
            _ => {}
        }
    }
    None
}

fn extract_test(attrs: &str) -> Result<String, String> {
    let idx = attrs
        .find("test")
        .ok_or_else(|| "conditional tag without test attribute".to_string())?;
    let rest = &attrs[idx + 4..];
    let rest = rest
        .trim_start()
        .strip_prefix('=')
        .ok_or("malformed test")?;
    let rest = rest.trim_start();
    let quote = rest.chars().next().ok_or("malformed test")?;
    if quote != '"' && quote != '\'' {
        return Err("unquoted test attribute".to_string());
    }
    let value = &rest[1..];
    let end = value.find(quote).ok_or("unterminated test attribute")?;
    Ok(normalize(&value[..end]))
}

/// Collapse a content span to a short text summary: tags stripped,
/// whitespace collapsed, truncated. None when nothing meaningful remains.
fn summarize(content: &str) -> Option<String> {
    let mut text = String::new();
    let mut in_tag = false;
    for c in content.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                text.push(' ');
            }
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    let collapsed = normalize(&text);
    if collapsed.is_empty() {
        return None;
    }
    Some(if collapsed.chars().count() > SUMMARY_CHARS {
        let truncated: String = collapsed.chars().take(SUMMARY_CHARS).collect();
        format!("{truncated}…")
    } else {
        collapsed
    })
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn line_of(source: &str, offset: usize) -> u32 {
    source[..offset].bytes().filter(|&b| b == b'\n').count() as u32 + 1
}

/// Enumerate render paths through a cluster and build its decision table.
/// Returns Ok(None) when the cluster contains no conditions.
fn build_table(nodes: &[Node]) -> Result<Option<DecisionTable>, String> {
    struct Builder {
        atoms: Vec<String>,
        paths: Vec<RenderPath>,
    }

    impl Builder {
        fn atom_id(&mut self, label: &str) -> Result<usize, String> {
            if let Some(id) = self.atoms.iter().position(|a| a == label) {
                return Ok(id);
            }
            if self.atoms.len() >= MAX_ATOMS {
                return Err(format!("more than {MAX_ATOMS} distinct conditions"));
            }
            self.atoms.push(label.to_string());
            Ok(self.atoms.len() - 1)
        }

        /// Assign atom=value on the path; false = infeasible (already pinned
        /// to the opposite value, e.g. the same EL reused).
        fn assign(
            &mut self,
            conds: &mut Vec<(usize, bool)>,
            label: &str,
            value: bool,
        ) -> Result<bool, String> {
            let id = self.atom_id(label)?;
            if let Some(&(_, existing)) = conds.iter().find(|(a, _)| *a == id) {
                return Ok(existing == value);
            }
            conds.push((id, value));
            Ok(true)
        }

        fn walk(
            &mut self,
            nodes: &[Node],
            idx: usize,
            conds: Vec<(usize, bool)>,
            rendered: Vec<String>,
        ) -> Result<(), String> {
            if self.paths.len() > MAX_PATHS {
                return Err(format!("more than {MAX_PATHS} paths"));
            }
            let Some(node) = nodes.get(idx) else {
                self.paths.push((conds, rendered));
                return Ok(());
            };
            match node {
                Node::Text(t) => {
                    let mut rendered = rendered;
                    rendered.push(t.clone());
                    self.walk(nodes, idx + 1, conds, rendered)
                }
                Node::If { test, body } => {
                    // true branch: render body, then continue
                    let mut t_conds = conds.clone();
                    if self.assign(&mut t_conds, test, true)? {
                        self.walk_into(body, nodes, idx, t_conds, rendered.clone())?;
                    }
                    // false branch: skip body
                    let mut f_conds = conds;
                    if self.assign(&mut f_conds, test, false)? {
                        self.walk(nodes, idx + 1, f_conds, rendered)?;
                    }
                    Ok(())
                }
                Node::Choose { whens, otherwise } => {
                    let mut prior: Vec<String> = Vec::new();
                    for (test, body) in whens {
                        let mut conds_k = conds.clone();
                        let mut feasible = true;
                        for earlier in &prior {
                            if !self.assign(&mut conds_k, earlier, false)? {
                                feasible = false;
                                break;
                            }
                        }
                        if feasible && self.assign(&mut conds_k, test, true)? {
                            self.walk_into(body, nodes, idx, conds_k, rendered.clone())?;
                        }
                        prior.push(test.clone());
                    }
                    // all whens false → otherwise (or nothing)
                    let mut conds_o = conds;
                    let mut feasible = true;
                    for earlier in &prior {
                        if !self.assign(&mut conds_o, earlier, false)? {
                            feasible = false;
                            break;
                        }
                    }
                    if feasible {
                        match otherwise {
                            Some(body) => self.walk_into(body, nodes, idx, conds_o, rendered)?,
                            None => self.walk(nodes, idx + 1, conds_o, rendered)?,
                        }
                    }
                    Ok(())
                }
            }
        }

        /// Walk a conditional body, then resume the sibling list after it.
        fn walk_into(
            &mut self,
            body: &[Node],
            siblings: &[Node],
            idx: usize,
            conds: Vec<(usize, bool)>,
            rendered: Vec<String>,
        ) -> Result<(), String> {
            // Enumerate the body's paths into a sub-builder sharing atoms,
            // then continue each at the next sibling.
            let mut sub = Builder {
                atoms: std::mem::take(&mut self.atoms),
                paths: Vec::new(),
            };
            sub.walk(body, 0, conds, rendered)?;
            self.atoms = sub.atoms;
            for (conds, rendered) in sub.paths {
                self.walk(siblings, idx + 1, conds, rendered)?;
            }
            Ok(())
        }
    }

    let mut b = Builder {
        atoms: Vec::new(),
        paths: Vec::new(),
    };
    b.walk(nodes, 0, Vec::new(), Vec::new())?;

    if b.atoms.is_empty() {
        return Ok(None);
    }

    // Content rendered on every path is unconditional — show only what the
    // conditions actually change (mirrors the Java extractor).
    let common: std::collections::HashSet<&String> =
        b.paths
            .iter()
            .skip(1)
            .fold(b.paths[0].1.iter().collect(), |acc, (_, rendered)| {
                let set: std::collections::HashSet<&String> = rendered.iter().collect();
                acc.intersection(&set).copied().collect()
            });

    let mut table = DecisionTable::new(b.atoms.clone());
    for (conds, rendered) in &b.paths {
        let distinct: Vec<&str> = rendered
            .iter()
            .filter(|t| !common.contains(t))
            .map(String::as_str)
            .collect();
        let outcome = if distinct.is_empty() {
            "(no conditional content)".to_string()
        } else {
            format!("renders: {}", distinct.join(" ⊕ "))
        };
        let mut inputs = vec![Tri::Any; b.atoms.len()];
        for &(atom, value) in conds {
            inputs[atom] = if value { Tri::True } else { Tri::False };
        }
        table.add_row(inputs, outcome).map_err(|e| e.to_string())?;
    }
    Ok(Some(table))
}

#[cfg(test)]
mod tests {
    use super::*;
    use specgraphen_logic::compress;

    fn extract(jsp: &str) -> TagExtraction {
        TagExtractor::new().extract(jsp)
    }

    fn single(jsp: &str) -> TagDecision {
        let mut e = extract(jsp);
        assert_eq!(e.clusters.len(), 1, "skipped: {:?}", e.skipped);
        e.clusters.remove(0)
    }

    #[test]
    fn simple_if_controls_content() {
        let d = single(
            r#"<div>
                <c:if test="${user.premium}">
                    <span>PREMIUM</span>
                </c:if>
            </div>"#,
        );
        assert_eq!(d.table.variables(), ["${user.premium}"]);
        let outcomes: Vec<_> = d.table.rows().iter().map(|r| r.outcome.as_str()).collect();
        assert!(
            outcomes.iter().any(|o| o.contains("PREMIUM")),
            "{outcomes:?}"
        );
        assert!(
            outcomes.contains(&"(no conditional content)"),
            "{outcomes:?}"
        );
    }

    #[test]
    fn choose_when_chain_is_sequentially_exclusive() {
        let d = single(
            r#"<c:choose>
                <c:when test="${type == 'A'}">alpha</c:when>
                <c:when test="${type == 'B'}">beta</c:when>
                <c:otherwise>other</c:otherwise>
            </c:choose>"#,
        );
        assert_eq!(d.table.rows().len(), 3);
        let beta = d
            .table
            .rows()
            .iter()
            .find(|r| r.outcome.contains("beta"))
            .unwrap();
        assert_eq!(beta.inputs, vec![Tri::False, Tri::True]);
    }

    #[test]
    fn identical_branches_compress_to_dead_condition() {
        // Both branches render the same thing — the condition is patch noise
        let d = single(
            r#"<c:choose>
                <c:when test="${flag}">same text</c:when>
                <c:otherwise>same text</c:otherwise>
            </c:choose>"#,
        );
        let c = compress(&d.table).unwrap();
        assert_eq!(c.dead_variables, vec!["${flag}"]);
    }

    #[test]
    fn nested_if_inside_when() {
        let d = single(
            r#"<c:choose>
                <c:when test="${member}">
                    hello
                    <c:if test="${birthday}">CAKE</c:if>
                </c:when>
                <c:otherwise>guest</c:otherwise>
            </c:choose>"#,
        );
        assert_eq!(d.table.variables().len(), 2);
        let cake = d
            .table
            .rows()
            .iter()
            .find(|r| r.outcome.contains("CAKE"))
            .unwrap();
        assert_eq!(cake.inputs, vec![Tri::True, Tri::True]);
        // guest path must not constrain the birthday atom
        let guest = d
            .table
            .rows()
            .iter()
            .find(|r| r.outcome.contains("guest"))
            .unwrap();
        assert_eq!(guest.inputs, vec![Tri::False, Tri::Any]);
    }

    #[test]
    fn repeated_el_expression_is_feasibility_tracked() {
        let jsp = r#"
            <c:if test="${on}">first</c:if>
            <c:if test="${on}">second</c:if>
        "#;
        // two top-level clusters share nothing; each is its own table
        let e = extract(jsp);
        assert_eq!(e.clusters.len(), 2);
    }

    #[test]
    fn scriptlet_inside_cluster_marks_incomplete() {
        let d = single(
            r#"<c:if test="${y}">
                <% if (request.getAttribute("x") != null) { %>legacy<% } %>
                modern
            </c:if>"#,
        );
        assert!(d.incomplete);
    }

    #[test]
    fn scriptlet_outside_cluster_does_not_taint_it() {
        let d = single(
            r#"<% if (request.getAttribute("x") != null) { %>
                legacy region, separate from the cluster below
            <% } %>
            <c:if test="${y}">modern</c:if>"#,
        );
        assert!(!d.incomplete);
    }

    #[test]
    fn other_c_tags_are_content() {
        let d = single(
            r#"<c:if test="${list != null}">
                <c:forEach items="${list}" var="x">row</c:forEach>
            </c:if>"#,
        );
        assert_eq!(d.table.variables(), ["${list != null}"]);
    }

    #[test]
    fn unbalanced_markup_is_skipped_not_panicking() {
        let e = extract(r#"<c:if test="${a}">never closed"#);
        assert!(e.clusters.is_empty());
        assert_eq!(e.skipped.len(), 1);
    }

    #[test]
    fn quoted_gt_in_el_does_not_end_the_tag() {
        let d = single(r#"<c:if test="${count > 3}">many</c:if>"#);
        assert_eq!(d.table.variables(), ["${count > 3}"]);
    }

    #[test]
    fn no_conditionals_means_no_clusters() {
        let e = extract("<div>plain</div>");
        assert!(e.clusters.is_empty());
        assert!(e.skipped.is_empty());
    }
}
