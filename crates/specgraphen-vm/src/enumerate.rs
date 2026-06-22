//! All-paths enumeration: a method's full **spec table**.
//!
//! Where [`crate::stepper::Stepper`] advances interactively and pauses at each
//! branch, this module drives that *same* stepper to completion across *every*
//! world-line, collecting each as a [`WorldLine`] — its pinned condition atoms,
//! its outcome (symbolic `return` value / `throw` / terminal call /
//! fall-through), the `this.*`/object **field writes** it performs, and the
//! observable calls along it. The result is a [`SpecTable`]: the method's
//! AS-IS behavior expressed as a piecewise function `conditions → (return,
//! state)`, recovered without running anything.
//!
//! Enumeration reuses the canonical stepper semantics (clone at each branch,
//! choose each option) so the table can never diverge from what interactive
//! stepping would show — drilling into any row reproduces it exactly.
//!
//! Values are kept **symbolic**: a `return`/field-write expression is expanded
//! by substituting the last write of each local it mentions ([`deep_resolve`]),
//! so `int t = a * r; return t;` reports `return (a * r)` rather than a concrete
//! number. There is no arithmetic evaluation and no SMT — unresolved calls and
//! opaque arithmetic stay as named symbolic terms, on purpose.

use std::collections::BTreeMap;

use crate::stepper::{Stepper, StopKind};
use crate::sym::SymError;

/// How a world-line ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `return <expr>` — the symbolic return value (empty for a bare `return;`).
    Return(String),
    /// `throw <expr>`.
    Throw(String),
    /// A process-terminating call (e.g. `System.exit(1)`).
    Exit(String),
    /// Ran off the end of the method with no explicit return.
    FallThrough,
}

/// One enumerated world-line of a method.
#[derive(Debug, Clone)]
pub struct WorldLine {
    /// Condition atoms pinned on this path, as `(atom-name, value)` in the
    /// order they were decided.
    pub rules: Vec<(String, bool)>,
    /// How the path ended.
    pub outcome: Outcome,
    /// State changes: assignment targets that are fields / object members
    /// (their text contains a `.`, e.g. `this.status`), with their symbolic
    /// value. Plain local variables are intermediate and excluded.
    pub field_writes: Vec<(String, String)>,
    /// Observable (non-logging) calls recorded along the path.
    pub calls: Vec<String>,
    /// True if the path stepped over an unmodeled construct that can itself
    /// exit the method — its outcome is not fully trustworthy.
    pub incomplete: bool,
}

/// A method's behavior as a decision table over its condition atoms.
#[derive(Debug, Clone)]
pub struct SpecTable {
    /// Condition columns: the union of every world-line's atoms, in first-seen
    /// order.
    pub atoms: Vec<String>,
    /// One row per world-line.
    pub world_lines: Vec<WorldLine>,
    /// `conds[i][j]` is world-line `i`'s value for atom `atoms[j]`:
    /// `Some(true|false)` if pinned, `None` ("any") if that atom is irrelevant
    /// on this path. Values are for the *bare* predicate `atoms[j]`.
    pub conds: Vec<Vec<Option<bool>>>,
    /// Per column (aligned to `atoms`): the predicate was only ever written
    /// negated in the source, so a reader is better served seeing it as `!atom`
    /// with its cells flipped — a legacy `if (!guard)` then reads with no mental
    /// inversion. The stored `conds` stay canonical; this is a display hint.
    pub display_negated: Vec<bool>,
    /// Set with a reason when enumeration hit a cap and the table is partial.
    pub truncated: Option<String>,
}

/// Hard caps so a pathological method can't enumerate forever. Mirrors the
/// batch decision-table extractor's limits.
const MAX_WORLD_LINES: usize = 4096;
const MAX_STEPS: usize = 500_000;

/// Drive `start` across every branch to completion, collecting all world-lines.
///
/// Takes the stepper by value and clones it at each branch (its history is
/// dropped per clone — enumeration never undoes). Returns the assembled
/// [`SpecTable`]; a hit cap is reported via [`SpecTable::truncated`] rather than
/// as an error, so a partial-but-useful table is still returned.
pub fn enumerate(start: Stepper) -> Result<SpecTable, SymError> {
    let mut world_lines: Vec<WorldLine> = Vec::new();
    // Atom name → (seen-positive, seen-negated), unioned across all world-lines.
    let mut polarity: BTreeMap<String, (bool, bool)> = BTreeMap::new();
    let mut truncated = None;
    let mut steps = 0usize;
    let mut work = vec![start];

    'outer: while let Some(mut s) = work.pop() {
        loop {
            if world_lines.len() >= MAX_WORLD_LINES {
                truncated = Some(format!("more than {MAX_WORLD_LINES} world-lines"));
                break 'outer;
            }
            if steps >= MAX_STEPS {
                truncated = Some(format!("more than {MAX_STEPS} steps"));
                break 'outer;
            }
            steps += 1;

            let stop = s.step()?;
            match stop.kind {
                StopKind::Stepped => continue,
                StopKind::Branch => {
                    let n = s.peek_branches().len();
                    // Push options in reverse so the worklist (a stack) pops
                    // them in source order — stable, readable row ordering.
                    for i in (0..n).rev() {
                        let mut child = s.clone();
                        child.forget_history();
                        child.choose(i)?;
                        work.push(child);
                    }
                    break;
                }
                StopKind::Terminated => {
                    world_lines.push(capture(&s, stop.outcome, &mut polarity));
                    break;
                }
                StopKind::FallThrough => {
                    world_lines.push(capture(&s, None, &mut polarity));
                    break;
                }
            }
        }
    }

    Ok(build_table(world_lines, polarity, truncated))
}

/// Snapshot the finished stepper as a [`WorldLine`], folding each pinned atom's
/// source polarity into the running `polarity` map.
fn capture(
    s: &Stepper,
    outcome: Option<String>,
    polarity: &mut BTreeMap<String, (bool, bool)>,
) -> WorldLine {
    let state = s.state();
    let atoms = s.atoms();
    let writes = &state.writes;

    let rules: Vec<(String, bool)> = state
        .conds
        .iter()
        .map(|&(a, v)| (atoms.name(a).to_string(), v))
        .collect();

    for &(id, _) in &state.conds {
        let (p, n) = atoms.polarity(id);
        let entry = polarity
            .entry(atoms.name(id).to_string())
            .or_insert((false, false));
        entry.0 |= p;
        entry.1 |= n;
    }

    let field_writes = writes
        .iter()
        .filter(|(target, _)| target.contains('.'))
        .map(|(target, value)| (target.clone(), deep_resolve(value, writes, RESOLVE_DEPTH)))
        .collect();

    let calls = state.calls.iter().cloned().collect();

    WorldLine {
        rules,
        outcome: classify(outcome, writes),
        field_writes,
        calls,
        incomplete: s.is_incomplete(),
    }
}

/// Turn a stepper outcome string into a typed, symbolically-resolved [`Outcome`].
fn classify(outcome: Option<String>, writes: &BTreeMap<String, String>) -> Outcome {
    let Some(s) = outcome else {
        return Outcome::FallThrough;
    };
    if s == "return" {
        Outcome::Return(String::new())
    } else if let Some(rest) = s.strip_prefix("return ") {
        Outcome::Return(deep_resolve(rest, writes, RESOLVE_DEPTH))
    } else if let Some(rest) = s.strip_prefix("throw ") {
        Outcome::Throw(deep_resolve(rest, writes, RESOLVE_DEPTH))
    } else {
        // A terminal call (System.exit(...) and friends) — leave as written.
        Outcome::Exit(s)
    }
}

/// Assemble world-lines into a column-aligned table.
fn build_table(
    world_lines: Vec<WorldLine>,
    polarity: BTreeMap<String, (bool, bool)>,
    truncated: Option<String>,
) -> SpecTable {
    let mut atoms: Vec<String> = Vec::new();
    for wl in &world_lines {
        for (name, _) in &wl.rules {
            if !atoms.iter().any(|a| a == name) {
                atoms.push(name.clone());
            }
        }
    }
    let conds = world_lines
        .iter()
        .map(|wl| {
            atoms
                .iter()
                .map(|a| wl.rules.iter().find(|(n, _)| n == a).map(|(_, v)| *v))
                .collect()
        })
        .collect();
    // A column reads better negated when its predicate was *only* ever written
    // under `!` (seen negated, never positive).
    let display_negated = atoms
        .iter()
        .map(|a| {
            let (pos, neg) = polarity.get(a).copied().unwrap_or((false, false));
            neg && !pos
        })
        .collect();
    SpecTable {
        atoms,
        world_lines,
        conds,
        display_negated,
        truncated,
    }
}

/// How deep to expand nested local definitions into a symbolic expression.
const RESOLVE_DEPTH: usize = 5;

/// Expand `expr` into a deeper symbolic form by replacing each whole-identifier
/// token with the last value written to it on this path, recursively (bounded
/// by `depth`). `int t = a * r; ... return t;` ⇒ `(a * r)`.
///
/// Conservative on purpose — it never produces something *less* informative
/// than the input:
/// - a member access (`x` in `obj.x`) and a call name (`f` in `f(...)`) are
///   left alone, so methods/fields aren't mis-substituted;
/// - a self-referential write (`i = i + 1`, or any `+=`/`-=` whose recorded
///   value mentions the target) is left as the bare name, never inlined into a
///   circular blob.
fn deep_resolve(expr: &str, writes: &BTreeMap<String, String>, depth: usize) -> String {
    if depth == 0 {
        return expr.to_string();
    }
    let bytes = expr.as_bytes();
    let mut out = String::with_capacity(expr.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if is_ident_start(c) {
            let start = i;
            i += 1;
            while i < bytes.len() && is_ident_char(bytes[i]) {
                i += 1;
            }
            let ident = &expr[start..i];
            let prev_is_dot = start > 0 && bytes[start - 1] == b'.';
            let next_is_call = {
                let mut j = i;
                while j < bytes.len() && bytes[j] == b' ' {
                    j += 1;
                }
                j < bytes.len() && bytes[j] == b'('
            };
            if !prev_is_dot && !next_is_call {
                if let Some(value) = writes.get(ident) {
                    if !value.is_empty() && !references_ident(value, ident) {
                        out.push('(');
                        out.push_str(&deep_resolve(value, writes, depth - 1));
                        out.push(')');
                        continue;
                    }
                }
            }
            out.push_str(ident);
        } else {
            // Copy a run of non-identifier bytes verbatim. Crucially this keeps
            // multibyte UTF-8 (e.g. Japanese string literals) intact — pushing
            // `byte as char` would Latin-1-mangle each byte into mojibake.
            // Identifier starts are ASCII, and every byte of a multibyte char is
            // a non-start, so the whole char rides this run; both bounds land on
            // char boundaries, so the slice never panics.
            let start = i;
            i += 1;
            while i < bytes.len() && !is_ident_start(bytes[i]) {
                i += 1;
            }
            out.push_str(&expr[start..i]);
        }
    }
    out
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$'
}

fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

/// Does `text` mention `ident` as a whole identifier token? (`.` is a boundary,
/// so over-matching only errs toward *not* substituting — the safe direction.)
fn references_ident(text: &str, ident: &str) -> bool {
    if ident.is_empty() {
        return false;
    }
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(rel) = text[from..].find(ident) {
        let start = from + rel;
        let end = start + ident.len();
        let before_ok = start == 0 || !is_ident_char(bytes[start - 1]);
        let after_ok = end == text.len() || !is_ident_char(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> tree_sitter::Tree {
        let mut p = tree_sitter::Parser::new();
        p.set_language(&tree_sitter_java::LANGUAGE.into()).unwrap();
        p.parse(src, None).unwrap()
    }

    fn method_body<'a>(tree: &'a tree_sitter::Tree) -> tree_sitter::Node<'a> {
        fn find(n: tree_sitter::Node) -> Option<tree_sitter::Node> {
            if n.kind() == "method_declaration" {
                return n.child_by_field_name("body");
            }
            let mut c = n.walk();
            for ch in n.children(&mut c) {
                if let Some(b) = find(ch) {
                    return Some(b);
                }
            }
            None
        }
        find(tree.root_node()).unwrap()
    }

    fn table_for(src: &str) -> SpecTable {
        let tree = parse(src);
        let bytes = src.as_bytes();
        let body = method_body(&tree);
        let stepper = Stepper::start(body, bytes, vec!["System.exit".to_string()], 64);
        enumerate(stepper).unwrap()
    }

    #[test]
    fn linear_method_is_one_world_line() {
        let t = table_for("class A { int m() { int x = 5; x = 7; return x; } }");
        assert_eq!(t.world_lines.len(), 1);
        assert_eq!(t.atoms.len(), 0);
        assert_eq!(t.world_lines[0].outcome, Outcome::Return("7".to_string()));
    }

    #[test]
    fn if_else_yields_two_world_lines_per_outcome() {
        let src = r#"class A { String m(boolean a) {
            if (a) { return "yes"; }
            return "no";
        } }"#;
        let t = table_for(src);
        assert_eq!(t.world_lines.len(), 2);
        assert_eq!(t.atoms, vec!["a".to_string()]);
        let outs: Vec<&Outcome> = t.world_lines.iter().map(|w| &w.outcome).collect();
        assert!(outs.contains(&&Outcome::Return("\"yes\"".to_string())));
        assert!(outs.contains(&&Outcome::Return("\"no\"".to_string())));
        // The "yes" world pins a == true; the "no" world a == false.
        let yes = t
            .world_lines
            .iter()
            .position(|w| w.outcome == Outcome::Return("\"yes\"".to_string()))
            .unwrap();
        assert_eq!(t.conds[yes], vec![Some(true)]);
    }

    #[test]
    fn return_value_is_symbolically_resolved_through_locals() {
        // The headline: `return t` where `t = a * r` and `r` is itself a local
        // expands to the full symbolic expression — no runtime, no SMT.
        let src = r#"class A { int m(int a, int base) {
            int r = base + 1;
            int t = a * r;
            return t;
        } }"#;
        let t = table_for(src);
        assert_eq!(t.world_lines.len(), 1);
        // `return t` substitutes t = `a * r` (stepper, one level), then
        // deep_resolve expands `r` = `base + 1`; `a` is a param with no write.
        assert_eq!(
            t.world_lines[0].outcome,
            Outcome::Return("a * (base + 1)".to_string())
        );
    }

    #[test]
    fn field_writes_are_captured_as_state_changes() {
        let src = r#"class A { void m(boolean active, int amount) {
            if (active) { this.status = "OPEN"; this.total = amount; }
            else { this.status = "CLOSED"; }
        } }"#;
        let t = table_for(src);
        // active=true world writes status+total; active=false writes status.
        let active = t
            .world_lines
            .iter()
            .find(|w| w.rules.iter().any(|(n, v)| n == "active" && *v))
            .unwrap();
        assert!(active
            .field_writes
            .iter()
            .any(|(k, v)| k == "this.status" && v == "\"OPEN\""));
        assert!(active
            .field_writes
            .iter()
            .any(|(k, v)| k == "this.total" && v == "amount"));
        let inactive = t
            .world_lines
            .iter()
            .find(|w| w.rules.iter().any(|(n, v)| n == "active" && !*v))
            .unwrap();
        assert!(inactive
            .field_writes
            .iter()
            .any(|(k, v)| k == "this.status" && v == "\"CLOSED\""));
        // Local `amount` is not a state change (no dot) — excluded.
        assert!(!active.field_writes.iter().any(|(k, _)| k == "amount"));
    }

    #[test]
    fn throw_is_a_distinct_outcome() {
        let src = r#"class A { int m(int x) {
            if (x == 0) { throw new IllegalArgumentException("zero"); }
            return x;
        } }"#;
        let t = table_for(src);
        let outs: Vec<&Outcome> = t.world_lines.iter().map(|w| &w.outcome).collect();
        assert!(outs
            .iter()
            .any(|o| matches!(o, Outcome::Throw(e) if e.contains("IllegalArgumentException"))));
    }

    #[test]
    fn self_referential_write_does_not_loop() {
        // `n += 1` records the whole `n += 1` text as n's value (a compound
        // assignment has no simple symbolic value). Resolving `return n` must
        // not recurse forever or produce a circular blob — the self-reference
        // guard stops it, leaving the honest `n += 1` text.
        let src = "class A { int m(int n) { n += 1; return n; } }";
        let t = table_for(src);
        assert_eq!(
            t.world_lines[0].outcome,
            Outcome::Return("n += 1".to_string())
        );
    }

    #[test]
    fn multibyte_literals_survive_resolution() {
        // Multibyte string literals (legacy Shift_JIS sources decoded to UTF-8)
        // must not be Latin-1-mangled into mojibake by deep_resolve. Generic
        // placeholder text — not from any real source.
        let mut w = BTreeMap::new();
        w.insert("icon".to_string(), "\"日本語テスト\"".to_string());
        assert_eq!(deep_resolve("icon", &w, 5), "(\"日本語テスト\")");
        // And through a full method: build a label string and return it.
        let src = r#"class A { String m(boolean bb) {
            String label = "";
            if (bb) { label = "あいうえお"; }
            return label;
        } }"#;
        let t = table_for(src);
        assert!(
            t.world_lines
                .iter()
                .any(|wl| matches!(&wl.outcome, Outcome::Return(v) if v.contains("あいうえお"))),
            "multibyte literal must survive: {:?}",
            t.world_lines.iter().map(|w| &w.outcome).collect::<Vec<_>>()
        );
    }

    #[test]
    fn deep_resolve_leaves_calls_and_members_alone() {
        let mut w = BTreeMap::new();
        w.insert("x".to_string(), "5".to_string());
        // `x` substituted; `x` as a member (`o.x`) and as a call (`x(...)`) not.
        assert_eq!(deep_resolve("x + 1", &w, 5), "(5) + 1");
        assert_eq!(deep_resolve("o.x + 1", &w, 5), "o.x + 1");
        assert_eq!(deep_resolve("x(1)", &w, 5), "x(1)");
    }

    #[test]
    fn negated_only_predicate_is_flagged_for_negated_display() {
        // `if (!flag)` — the predicate is only ever written negated, so its
        // column should display as `!flag` (cells flipped by the consumer).
        let src = r#"class A { int m(boolean flag) {
            if (!flag) { return 1; }
            return 0;
        } }"#;
        let t = table_for(src);
        assert_eq!(t.atoms, vec!["flag".to_string()]);
        assert_eq!(t.display_negated, vec![true]);
    }

    #[test]
    fn positively_used_predicate_is_not_flagged() {
        // Even though it also appears negated, a positive occurrence means we
        // keep the column positive (mixed → prefer positive, never ambiguous).
        let src = r#"class A { int m(boolean flag) {
            if (flag) { return 1; }
            if (!flag) { return 2; }
            return 0;
        } }"#;
        let t = table_for(src);
        assert_eq!(t.atoms, vec!["flag".to_string()]);
        assert_eq!(t.display_negated, vec![false]);
    }

    #[test]
    fn switch_enumerates_a_world_line_per_case() {
        let src = r#"class A { String m(String code) {
            switch (code) {
                case "A": return "alpha";
                case "B": return "beta";
                default: return "other";
            }
        } }"#;
        let t = table_for(src);
        assert_eq!(t.world_lines.len(), 3, "two cases + default");
        let outs: Vec<&Outcome> = t.world_lines.iter().map(|w| &w.outcome).collect();
        assert!(outs.contains(&&Outcome::Return("\"alpha\"".to_string())));
        assert!(outs.contains(&&Outcome::Return("\"beta\"".to_string())));
        assert!(outs.contains(&&Outcome::Return("\"other\"".to_string())));
    }

    #[test]
    fn loop_offers_enter_and_skip_world_lines() {
        let src = r#"class A { String m(java.util.Iterator<String> it) {
            while (it.hasNext()) { return "in-loop"; }
            return "after";
        } }"#;
        let t = table_for(src);
        let outs: Vec<&Outcome> = t.world_lines.iter().map(|w| &w.outcome).collect();
        assert!(outs.contains(&&Outcome::Return("\"in-loop\"".to_string())));
        assert!(outs.contains(&&Outcome::Return("\"after\"".to_string())));
    }
}
