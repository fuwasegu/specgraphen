//! Symbolic-execution semantics over a Java AST.
//!
//! This is the shared core used both by batch decision-table extraction and
//! by the interactive [`crate::stepper::Stepper`]. It is deliberately free of
//! any driver concerns (path collection, merging, caps): it only knows how to
//! evaluate a condition, record a write, and recognise a process-terminating
//! call against a per-run symbolic [`SymState`].
//!
//! Conditions are atomized by normalized expression text — same text means the
//! same boolean variable — interned in an [`AtomTable`]. `&&`/`||` are
//! decomposed with short-circuit path semantics and `!` flips the value. An
//! atom already pinned on a path follows its assigned value, so infeasible
//! combinations are never produced — but a write to a variable a pinned atom
//! references **invalidates** that pin ([`record_write`]/[`bind_local`]), so
//! the common `flag = true; if (flag)` pattern re-tests against the new value
//! instead of contradicting it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Per-path symbolic state: condition-atom assignments plus observable effects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SymState {
    /// Condition atoms decided on this path: (atom id, value).
    pub conds: Vec<(usize, bool)>,
    /// Assignment target → last assigned value text along this path.
    pub writes: BTreeMap<String, String>,
    /// Statement-level method calls observed along this path (error exits,
    /// state mutations) — logging is excluded.
    pub calls: BTreeSet<String>,
}

/// Interns condition-atom labels to small indices, with a hard cap.
#[derive(Debug, Clone)]
pub struct AtomTable {
    names: Vec<String>,
    max: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymError {
    /// More than `max` distinct condition atoms were interned.
    TooManyAtoms { max: usize },
    /// The AST had a shape the evaluator could not interpret.
    Malformed(&'static str),
}

impl fmt::Display for SymError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Wording preserved so downstream skip-reason checks keep matching.
            Self::TooManyAtoms { max } => write!(f, "more than {max} distinct conditions"),
            Self::Malformed(what) => write!(f, "{what}"),
        }
    }
}

impl std::error::Error for SymError {}

impl AtomTable {
    pub fn new(max: usize) -> Self {
        Self {
            names: Vec::new(),
            max,
        }
    }

    /// Intern a label, returning its stable id. Errors past the cap.
    pub fn intern(&mut self, label: &str) -> Result<usize, SymError> {
        if let Some(id) = self.names.iter().position(|a| a == label) {
            return Ok(id);
        }
        if self.names.len() >= self.max {
            return Err(SymError::TooManyAtoms { max: self.max });
        }
        self.names.push(label.to_string());
        Ok(self.names.len() - 1)
    }

    pub fn name(&self, id: usize) -> &str {
        &self.names[id]
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Look up an existing atom id without interning.
    pub fn id_of(&self, label: &str) -> Option<usize> {
        self.names.iter().position(|a| a == label)
    }
}

/// Condition evaluator: borrows the source bytes and the atom table.
pub struct Evaluator<'a> {
    pub src: &'a [u8],
    pub atoms: &'a mut AtomTable,
}

impl Evaluator<'_> {
    /// Evaluate a condition with short-circuit path semantics, returning every
    /// `(state, truth-value)` pair it can produce on the given path.
    pub fn eval(
        &mut self,
        node: tree_sitter::Node,
        state: SymState,
    ) -> Result<Vec<(SymState, bool)>, SymError> {
        match node.kind() {
            "parenthesized_expression" => {
                let inner = named_children(node);
                let inner = inner
                    .first()
                    .ok_or(SymError::Malformed("empty parentheses"))?;
                self.eval(*inner, state)
            }
            "unary_expression" => {
                let op = node
                    .child_by_field_name("operator")
                    .map(|n| text(n, self.src));
                let operand = node
                    .child_by_field_name("operand")
                    .ok_or(SymError::Malformed("unary without operand"))?;
                if op.as_deref() == Some("!") {
                    Ok(self
                        .eval(operand, state)?
                        .into_iter()
                        .map(|(s, v)| (s, !v))
                        .collect())
                } else {
                    self.atom(node, state)
                }
            }
            "binary_expression" => {
                let op = node
                    .child_by_field_name("operator")
                    .map(|n| text(n, self.src))
                    .unwrap_or_default();
                match op.as_str() {
                    "&&" => self.short_circuit(node, state, true),
                    "||" => self.short_circuit(node, state, false),
                    _ => self.atom(node, state),
                }
            }
            "true" => Ok(vec![(state, true)]),
            "false" => Ok(vec![(state, false)]),
            _ => self.atom(node, state),
        }
    }

    /// `&&` (and=true): left=false short-circuits; left=true continues into
    /// the right operand. `||` is the mirror image.
    fn short_circuit(
        &mut self,
        node: tree_sitter::Node,
        state: SymState,
        and: bool,
    ) -> Result<Vec<(SymState, bool)>, SymError> {
        let left = node
            .child_by_field_name("left")
            .ok_or(SymError::Malformed("missing lhs"))?;
        let right = node
            .child_by_field_name("right")
            .ok_or(SymError::Malformed("missing rhs"))?;

        let mut results = Vec::new();
        for (s, v) in self.eval(left, state)? {
            if v == and {
                results.extend(self.eval(right, s)?);
            } else {
                results.push((s, v));
            }
        }
        Ok(results)
    }

    /// Treat `node` as an atomic condition: same normalized text → same atom.
    /// A path that already pinned the atom follows that value (feasibility).
    fn atom(
        &mut self,
        node: tree_sitter::Node,
        state: SymState,
    ) -> Result<Vec<(SymState, bool)>, SymError> {
        let label = normalize(&text(node, self.src));
        let id = self.atoms.intern(&label)?;

        if let Some(&(_, value)) = state.conds.iter().find(|(a, _)| *a == id) {
            return Ok(vec![(state, value)]);
        }

        // A recorded concrete write can settle the atom outright, even when no
        // condition interned it before the assignment (so the re-pin in
        // `record_write` never fired): `flag = true; … if (!flag)` and
        // `x = "A"; … if (x.equals("A"))` follow the assigned value instead
        // of re-forking into a world that contradicts the variable table.
        if let Some(value) = resolved_by_write(&label, &state) {
            let mut st = state;
            st.conds.push((id, value));
            return Ok(vec![(st, value)]);
        }

        // Mutual exclusion: `X.equals(c1)` / `X == c1` is false if a sibling
        // `X.equals(c2)` / `X == c2` (different constant) is already pinned
        // true — a value can't equal two distinct constants. Without this the
        // two equals-predicates are independent booleans and an UNSAT path
        // (`X=="0"` ∧ `X=="3"`) can form, mis-resolving a later `||` over them.
        if let Some((subj, c1)) = equality_constant(&label) {
            let contradicted = state.conds.iter().any(|&(a, v)| {
                v && a != id
                    && equality_constant(self.atoms.name(a))
                        .is_some_and(|(s2, c2)| s2 == subj && c2 != c1)
            });
            if contradicted {
                let mut st = state;
                st.conds.push((id, false));
                return Ok(vec![(st, false)]);
            }
        }

        let mut true_state = state.clone();
        true_state.conds.push((id, true));
        let mut false_state = state;
        false_state.conds.push((id, false));
        Ok(vec![(true_state, true), (false_state, false)])
    }

    /// Try to pin `label = value` on a path. Returns `false` when the path
    /// already pinned the atom to the opposite value (infeasible fork).
    pub fn assign_atom(
        &mut self,
        label: &str,
        value: bool,
        state: &mut SymState,
    ) -> Result<bool, SymError> {
        let id = self.atoms.intern(label)?;
        if let Some(&(_, existing)) = state.conds.iter().find(|(a, _)| *a == id) {
            return Ok(existing == value);
        }
        state.conds.push((id, value));
        Ok(true)
    }
}

/// Record an assignment / increment / observable call as a symbolic effect on
/// `state`. Anything else is ignored — its side effects are not modeled.
///
/// Crucially, writing a variable also **invalidates condition atoms that
/// reference it**: feasibility-pinning assumes a condition's value is stable,
/// but legacy code mutates a flag then re-tests it (`flag = true; if (flag)`).
/// Without this, the stale pin contradicts the new value. A literal boolean
/// assignment re-pins the bare-variable atom to the assigned value.
pub fn record_write(expr: tree_sitter::Node, src: &[u8], atoms: &AtomTable, state: &mut SymState) {
    match expr.kind() {
        "assignment_expression" => {
            let (Some(left), Some(right)) = (
                expr.child_by_field_name("left"),
                expr.child_by_field_name("right"),
            ) else {
                return;
            };
            let target = normalize(&text(left, src));
            let operator = expr
                .child_by_field_name("operator")
                .map(|n| text(n, src))
                .unwrap_or_default();
            // Compound assignment (`+=` …) has no simple value; keep the whole
            // expression text — outcomes still distinguish the paths.
            let value = if operator == "=" {
                normalize(&text(right, src))
            } else {
                normalize(&text(expr, src))
            };
            state.writes.insert(target.clone(), value.clone());
            invalidate(state, atoms, &target);
            // Precision: `x = true/false` re-pins the bare-variable atom so a
            // later `if (x)` follows the assigned value instead of re-forking.
            if operator == "=" && (value == "true" || value == "false") {
                if let Some(id) = atoms.id_of(&target) {
                    state.conds.retain(|(i, _)| *i != id);
                    state.conds.push((id, value == "true"));
                }
            }
        }
        "update_expression" => {
            // i++ / --i: opaque value, but an observable write
            let whole = normalize(&text(expr, src));
            let target = whole.trim_matches(['+', '-', ' ']).to_string();
            if !target.is_empty() {
                state.writes.insert(target.clone(), whole);
                invalidate(state, atoms, &target);
            }
        }
        "method_invocation" => {
            // Statement-level calls (error exits, state mutations) are
            // observable behavior. Logging is excluded: not business behavior,
            // and per-branch log messages would explode the outcome alphabet.
            let call = normalize(&text(expr, src));
            if !is_logging_call(&call) {
                state.calls.insert(call);
            }
        }
        _ => {}
    }
}

/// Bind a declared local (`T name = value`): record the write and invalidate
/// any condition atom that referenced `name` (a redeclaration changes it).
pub fn bind_local(name: &str, value: &str, atoms: &AtomTable, state: &mut SymState) {
    state.writes.insert(name.to_string(), value.to_string());
    invalidate(state, atoms, name);
}

/// Drop condition atoms whose label textually references variable `var` — their
/// pinned value is no longer trustworthy after `var` was written or rebound
/// (e.g. a `for (T var : …)` loop variable taking a new element each iteration).
pub fn invalidate(state: &mut SymState, atoms: &AtomTable, var: &str) {
    state
        .conds
        .retain(|(id, _)| !references(atoms.name(*id), var));
}

/// Recognize an equality-against-constant atom: `SUBJ.equals(LIT)`,
/// `SUBJ == LIT`, or `LIT == SUBJ` where LIT is a string/char/number literal.
/// Returns `(subject, literal)`. Used to enforce that one subject can't equal
/// two distinct constants at once.
fn equality_constant(label: &str) -> Option<(&str, &str)> {
    if let Some(idx) = label.find(".equals(") {
        if let Some(arg) = label[idx + ".equals(".len()..].strip_suffix(')') {
            let subject = &label[..idx];
            if !subject.is_empty() && is_literal(arg) {
                return Some((subject, arg));
            }
        }
    }
    if let Some(idx) = label.find("==") {
        let lhs = label[..idx].trim();
        let rhs = label[idx + 2..].trim();
        if !lhs.is_empty() && !rhs.is_empty() {
            if is_literal(rhs) && !is_literal(lhs) {
                return Some((lhs, rhs));
            }
            if is_literal(lhs) && !is_literal(rhs) {
                return Some((rhs, lhs));
            }
        }
    }
    None
}

/// If a recorded concrete write determines this atom's truth, return it.
/// Covers a bare boolean variable assigned `true`/`false`, and an
/// equality-against-constant atom whose subject was last assigned a literal
/// (`x = "A"` ⇒ `x.equals("A")` is true, `x.equals("B")` is false).
/// A non-literal last write (a method call, another variable) is unknowable,
/// so the atom keeps forking.
fn resolved_by_write(label: &str, state: &SymState) -> Option<bool> {
    if let Some((subj, lit)) = equality_constant(label) {
        let v = state.writes.get(subj)?;
        return is_literal(v).then(|| v == lit);
    }
    match state.writes.get(label).map(String::as_str) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

/// A string/char/number literal token (heuristic, on normalized text).
fn is_literal(s: &str) -> bool {
    let s = s.trim();
    s.starts_with('"')
        || s.starts_with('\'')
        || s.chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit() || c == '-')
}

/// Does `label` mention `var` as a whole identifier token? `.` counts as a
/// boundary, so `status` matches in `status.equals("X")`. Over-matching only
/// causes a sound re-fork, so the check errs toward inclusion.
fn references(label: &str, var: &str) -> bool {
    if var.is_empty() {
        return false;
    }
    let is_ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let bytes = label.as_bytes();
    let mut from = 0;
    while let Some(rel) = label[from..].find(var) {
        let start = from + rel;
        let end = start + var.len();
        let before_ok = start == 0 || !is_ident(bytes[start - 1]);
        let after_ok = end == label.len() || !is_ident(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// Does this invocation match a configured process-terminating callee?
/// Matches the bare name (`abortOnError`) or `receiver.name` (`System.exit`).
pub fn is_terminal_call(
    invocation: tree_sitter::Node,
    src: &[u8],
    terminal_calls: &[String],
) -> bool {
    let Some(name) = invocation.child_by_field_name("name") else {
        return false;
    };
    let name = text(name, src);
    let dotted = invocation
        .child_by_field_name("object")
        .map(|o| format!("{}.{}", text(o, src), name));
    terminal_calls
        .iter()
        .any(|t| *t == name || Some(t.as_str()) == dotted.as_deref())
}

/// Logging receivers conventional in (legacy) Java. Branches that only log are
/// treated as having no observable effect.
const LOGGING_PREFIXES: &[&str] = &[
    "logger.",
    "log.",
    "LOG.",
    "Logger.",
    "System.out.",
    "System.err.",
];

pub fn is_logging_call(call: &str) -> bool {
    LOGGING_PREFIXES.iter().any(|p| call.starts_with(p))
}

/// One modelable `switch` group: the case values it matches (empty when it is
/// the `default`), and its body statements with any trailing `break` stripped.
#[derive(Debug, Clone)]
pub struct SwitchGroup<'a> {
    pub values: Vec<String>,
    pub is_default: bool,
    pub body: Vec<tree_sitter::Node<'a>>,
}

/// A `switch` reduced to else-if-chain semantics: the subject expression and
/// its groups in source order. Produced only for switches that can be modeled
/// faithfully (see [`parse_switch`]).
#[derive(Debug, Clone)]
pub struct SwitchModel<'a> {
    pub subject: String,
    pub groups: Vec<SwitchGroup<'a>>,
}

/// Reduce a `switch` to modelable groups, or `None` when it cannot be modeled
/// faithfully — i.e. a group falls through into the next (no break/return), or
/// has a conditional `break`. Stacked labels (`case 1: case 2: ...`) fold into
/// the group that carries the body. Both consumers (batch enumeration and the
/// interactive stepper) share this so their switch semantics never diverge.
pub fn parse_switch<'a>(node: tree_sitter::Node<'a>, src: &[u8]) -> Option<SwitchModel<'a>> {
    let cond = node.child_by_field_name("condition")?;
    let subject = normalize(text(cond, src).trim_matches(['(', ')']));
    let body = node.child_by_field_name("body")?;

    // Phase 1: collect raw groups (case values — empty = default, statements
    // including any trailing break, arrow-style flag).
    struct RawGroup<'t> {
        values: Vec<String>,
        is_default: bool,
        stmts: Vec<tree_sitter::Node<'t>>,
        is_rule: bool,
    }
    let mut raw: Vec<RawGroup> = Vec::new();
    for child in named_children(body) {
        match child.kind() {
            "switch_block_statement_group" | "switch_rule" => {
                let mut group = RawGroup {
                    values: Vec::new(),
                    is_default: false,
                    stmts: Vec::new(),
                    is_rule: child.kind() == "switch_rule",
                };
                for part in named_children(child) {
                    if part.kind() == "switch_label" {
                        let exprs = named_children(part);
                        if exprs.is_empty() {
                            group.is_default = true; // `default:`
                        }
                        for e in exprs {
                            group.values.push(normalize(&text(e, src)));
                        }
                    } else {
                        group.stmts.push(part);
                    }
                }
                if group.values.is_empty() && !group.is_default {
                    return None; // unexpected shape
                }
                raw.push(group);
            }
            _ => {}
        }
    }
    if raw.is_empty() {
        return None;
    }

    // Phase 2: stacked labels (`case 1:` with no statements) fold their values
    // into the next labeled group that carries a body.
    let mut merged: Vec<RawGroup> = Vec::new();
    let mut pending_values: Vec<String> = Vec::new();
    let mut pending_default = false;
    let last_index = raw.len() - 1;
    for (i, mut group) in raw.into_iter().enumerate() {
        if !group.is_rule && group.stmts.is_empty() && i != last_index {
            pending_values.append(&mut group.values);
            pending_default |= group.is_default;
            continue;
        }
        group.values = pending_values.drain(..).chain(group.values).collect();
        group.is_default |= std::mem::take(&mut pending_default);
        merged.push(group);
    }

    // Phase 3: faithfulness check per executable group + strip trailing break.
    let mut groups: Vec<SwitchGroup> = Vec::new();
    let merged_len = merged.len();
    for (i, mut group) in merged.into_iter().enumerate() {
        if !group.is_rule {
            let breaks = count_breaks(&group.stmts);
            let ends_with_break = group
                .stmts
                .last()
                .is_some_and(|s| s.kind() == "break_statement");
            if breaks > 1 || (breaks == 1 && !ends_with_break) {
                return None; // conditional break
            }
            if ends_with_break {
                group.stmts.pop();
            } else {
                let exits_always = group.stmts.last().is_some_and(|s| {
                    s.kind() == "return_statement" || s.kind() == "throw_statement"
                });
                // fall-through to the END of the switch is fine; into the next
                // group is not modeled.
                if !exits_always && i + 1 != merged_len {
                    return None;
                }
            }
        }
        groups.push(SwitchGroup {
            values: group.values,
            is_default: group.is_default,
            body: group.stmts,
        });
    }

    Some(SwitchModel { subject, groups })
}

/// Count `break` statements binding to the enclosing switch (not descending
/// into nested switches/loops, whose breaks bind there).
fn count_breaks(stmts: &[tree_sitter::Node]) -> usize {
    fn count(node: tree_sitter::Node) -> usize {
        match node.kind() {
            "break_statement" => 1,
            "switch_expression"
            | "switch_statement"
            | "while_statement"
            | "for_statement"
            | "enhanced_for_statement"
            | "do_statement" => 0,
            _ => {
                let mut cursor = node.walk();
                let children: Vec<_> = node.children(&mut cursor).collect();
                children.into_iter().map(count).sum()
            }
        }
    }
    stmts.iter().map(|&s| count(s)).sum()
}

/// Named children, skipping comments (comments never carry semantics).
pub fn named_children(node: tree_sitter::Node) -> Vec<tree_sitter::Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|n| n.kind() != "line_comment" && n.kind() != "block_comment")
        .collect()
}

pub fn text(node: tree_sitter::Node, src: &[u8]) -> String {
    node.utf8_text(src).unwrap_or_default().to_string()
}

/// Collapse runs of whitespace so logically-equal expressions atomize equally.
pub fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> tree_sitter::Tree {
        let mut p = tree_sitter::Parser::new();
        p.set_language(&tree_sitter_java::LANGUAGE.into()).unwrap();
        p.parse(src, None).unwrap()
    }

    /// Find the first node of `kind` in the tree.
    fn find<'a>(node: tree_sitter::Node<'a>, kind: &str) -> Option<tree_sitter::Node<'a>> {
        if node.kind() == kind {
            return Some(node);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(found) = find(child, kind) {
                return Some(found);
            }
        }
        None
    }

    fn eval_cond(src: &str) -> (Vec<(SymState, bool)>, AtomTable) {
        let wrapped = format!("class A {{ void m() {{ if ({src}) {{}} }} }}");
        let tree = parse(&wrapped);
        let bytes = wrapped.as_bytes();
        let cond = find(tree.root_node(), "if_statement")
            .unwrap()
            .child_by_field_name("condition")
            .unwrap();
        // unwrap the parenthesized_expression
        let inner = named_children(cond);
        let cond = inner.first().copied().unwrap_or(cond);
        let mut atoms = AtomTable::new(16);
        let results = {
            let mut ev = Evaluator {
                src: bytes,
                atoms: &mut atoms,
            };
            ev.eval(cond, SymState::default()).unwrap()
        };
        (results, atoms)
    }

    #[test]
    fn single_atom_forks_true_false() {
        let (r, atoms) = eval_cond("a");
        assert_eq!(r.len(), 2);
        assert_eq!(atoms.names(), ["a"]);
        assert!(r.iter().any(|(_, v)| *v));
        assert!(r.iter().any(|(_, v)| !*v));
    }

    #[test]
    fn negation_flips() {
        let (r, _) = eval_cond("!a");
        let t = r.iter().find(|(_, v)| *v).unwrap();
        // value true means the atom `a` was false
        assert_eq!(t.0.conds, vec![(0, false)]);
    }

    #[test]
    fn and_short_circuits() {
        // a && b: a=false has no b; a=true forks b
        let (r, atoms) = eval_cond("a && b");
        assert_eq!(atoms.names(), ["a", "b"]);
        let false_paths: Vec<_> = r.iter().filter(|(_, v)| !*v).collect();
        // a=false (b unconstrained) and a=true,b=false
        assert!(false_paths.iter().any(|(s, _)| s.conds == vec![(0, false)]));
    }

    #[test]
    fn distinct_equals_constants_are_mutually_exclusive() {
        // s.equals("A") true ⇒ s.equals("B") is false (a value can't be both).
        let wrapped = r#"class A { void m(String s) { if (s.equals("A") || s.equals("B")) {} } }"#;
        let tree = parse(wrapped);
        let bytes = wrapped.as_bytes();
        let cond = find(tree.root_node(), "if_statement")
            .unwrap()
            .child_by_field_name("condition")
            .unwrap();
        let inner = named_children(cond);
        let or = inner.first().copied().unwrap_or(cond);
        let mut atoms = AtomTable::new(16);
        let mut ev = Evaluator {
            src: bytes,
            atoms: &mut atoms,
        };
        let results = ev.eval(or, SymState::default()).unwrap();
        // No world may pin both equals("A") and equals("B") true.
        let a = atoms.id_of("s.equals(\"A\")");
        let b = atoms.id_of("s.equals(\"B\")");
        for (state, _) in &results {
            let a_true = a.is_some_and(|id| state.conds.contains(&(id, true)));
            let b_true = b.is_some_and(|id| state.conds.contains(&(id, true)));
            assert!(!(a_true && b_true), "UNSAT world: s == A and s == B");
        }
    }

    #[test]
    fn repeated_atom_is_feasibility_pinned() {
        // a && a: second `a` follows the first's value, no new fork
        let (r, atoms) = eval_cond("a && a");
        assert_eq!(atoms.names(), ["a"]);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn atom_cap_is_enforced() {
        let mut atoms = AtomTable::new(2);
        atoms.intern("x").unwrap();
        atoms.intern("y").unwrap();
        assert_eq!(atoms.intern("z"), Err(SymError::TooManyAtoms { max: 2 }));
        assert_eq!(
            SymError::TooManyAtoms { max: 2 }.to_string(),
            "more than 2 distinct conditions"
        );
    }

    #[test]
    fn record_write_tracks_assignment_and_call() {
        let src = "class A { void m() { x = 5; obj.doThing(); logger.info(\"x\"); } }";
        let tree = parse(src);
        let bytes = src.as_bytes();
        let mut state = SymState::default();
        let atoms = AtomTable::new(16);
        // walk to the method body statements
        let body = find(tree.root_node(), "block").unwrap();
        for stmt in named_children(body) {
            if let Some(expr) = named_children(stmt).first() {
                record_write(*expr, bytes, &atoms, &mut state);
            }
        }
        assert_eq!(state.writes.get("x"), Some(&"5".to_string()));
        assert!(state.calls.iter().any(|c| c.contains("doThing")));
        assert!(!state.calls.iter().any(|c| c.contains("logger")));
    }

    #[test]
    fn terminal_call_detection() {
        let src = "class A { void m() { System.exit(1); bail(); } }";
        let tree = parse(src);
        let bytes = src.as_bytes();
        let body = find(tree.root_node(), "block").unwrap();
        let calls: Vec<_> = named_children(body)
            .iter()
            .filter_map(|s| named_children(*s).first().copied())
            .filter(|e| e.kind() == "method_invocation")
            .collect();
        assert!(is_terminal_call(
            calls[0],
            bytes,
            &["System.exit".to_string()]
        ));
        assert!(!is_terminal_call(
            calls[1],
            bytes,
            &["System.exit".to_string()]
        ));
        assert!(is_terminal_call(calls[1], bytes, &["bail".to_string()]));
    }
}
