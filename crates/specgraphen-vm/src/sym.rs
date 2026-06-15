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
//! combinations are never produced.

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
pub fn record_write(expr: tree_sitter::Node, src: &[u8], state: &mut SymState) {
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
            state.writes.insert(target, value);
        }
        "update_expression" => {
            // i++ / --i: opaque value, but an observable write
            let whole = normalize(&text(expr, src));
            let target = whole.trim_matches(['+', '-', ' ']).to_string();
            if !target.is_empty() {
                state.writes.insert(target, whole);
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
        let mut cursor = tree.root_node().walk();
        // walk to the method body statements
        let body = find(tree.root_node(), "block").unwrap();
        for stmt in named_children(body) {
            if let Some(expr) = named_children(stmt).first() {
                record_write(*expr, bytes, &mut state);
            }
        }
        let _ = &mut cursor;
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
