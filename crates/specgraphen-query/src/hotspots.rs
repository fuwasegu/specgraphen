use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use specgraphen_model::{JavaEntityType, SpaceData};

#[derive(Debug, Serialize, Deserialize)]
pub struct HotspotsResult {
    pub hotspots: Vec<Hotspot>,
    pub methods_analyzed: usize,
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Hotspot {
    pub fqn: String,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    /// Non-empty, non-comment lines in the method body.
    pub loc: usize,
    /// Approximate cyclomatic complexity: decision points + 1.
    pub complexity: usize,
}

pub fn hotspots(
    space_data: &SpaceData,
    source_files: &HashMap<String, String>,
    limit: usize,
) -> Result<HotspotsResult> {
    if source_files.is_empty() {
        anyhow::bail!("hotspots requires source files (start the server with --source-root)");
    }

    let mut spots = Vec::new();
    let mut analyzed = 0;

    for entity in &space_data.entities {
        if !matches!(
            entity.entity_type,
            JavaEntityType::Method | JavaEntityType::Constructor
        ) {
            continue;
        }
        let Some(content) = source_files.get(&entity.witness.file) else {
            continue;
        };
        let start = entity.witness.start_line as usize;
        let end = entity.witness.end_line as usize;
        if start == 0 || end < start {
            continue;
        }
        analyzed += 1;

        let body: Vec<&str> = content
            .lines()
            .skip(start - 1)
            .take(end - start + 1)
            .collect();
        let loc = body
            .iter()
            .filter(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with("//") && !t.starts_with('*') && !t.starts_with("/*")
            })
            .count();
        let complexity = 1 + body.iter().map(|l| decision_points(l)).sum::<usize>();

        spots.push(Hotspot {
            fqn: entity.fqn.clone(),
            file: entity.witness.file.clone(),
            start_line: entity.witness.start_line,
            end_line: entity.witness.end_line,
            loc,
            complexity,
        });
    }

    spots.sort_by(|a, b| b.complexity.cmp(&a.complexity).then(b.loc.cmp(&a.loc)));
    spots.truncate(limit);

    Ok(HotspotsResult {
        hotspots: spots,
        methods_analyzed: analyzed,
        note: "Complexity is approximated from decision-point keywords (if/for/while/case/catch, \
               &&, ||) in the method body. Use as a refactoring triage signal, not a metric of \
               record."
            .to_string(),
    })
}

fn decision_points(line: &str) -> usize {
    let mut n = 0;
    for kw in ["if", "for", "while", "case", "catch"] {
        n += count_keyword(line, kw);
    }
    n + line.matches("&&").count() + line.matches("||").count()
}

fn count_keyword(line: &str, kw: &str) -> usize {
    let mut count = 0;
    let mut start = 0;
    while let Some(pos) = line[start..].find(kw) {
        let abs = start + pos;
        let before_ok = abs == 0
            || line[..abs]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        let after = abs + kw.len();
        let after_ok = line[after..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        if before_ok && after_ok {
            count += 1;
        }
        start = abs + kw.len();
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_decision_points_with_boundaries() {
        assert_eq!(decision_points("if (a && b) {"), 2);
        assert_eq!(decision_points("notifyAll(); // modifier"), 0);
        assert_eq!(decision_points("} catch (Exception e) {"), 1);
        assert_eq!(decision_points("String gift = forty;"), 0);
    }
}
