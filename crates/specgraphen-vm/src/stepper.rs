//! Resumable, runtime-less symbolic stepper over one Java method body.
//!
//! Where batch extraction enumerates every path at once, the stepper advances
//! one statement at a time and *pauses* at each branch, presenting the feasible
//! world-lines for a human (or agent) to choose. Because nothing actually runs,
//! every state is an immutable snapshot — so undo is free.
//!
//! Control flow is an explicit stack of [`Frame`]s (statement list + program
//! counter), so there is no host-stack recursion to suspend: the whole
//! execution state is plain data that can be snapshotted, serialized to a GUI,
//! or driven from an MCP tool.
//!
//! Scope (v1): models `if`/`else`, blocks, sequential statements, assignments,
//! and terminal calls. `switch`, loops, and `try` are stepped over as opaque
//! statements (their effects are not modeled) and flagged via [`Stop::opaque`].
//! Method calls are recorded as effects, not descended into (no cross-method
//! step-into yet).

use crate::sym::{self, AtomTable, Evaluator, SymError, SymState};

/// One control-flow frame: a statement list and the index about to execute.
#[derive(Debug, Clone)]
pub struct Frame<'a> {
    stmts: Vec<tree_sitter::Node<'a>>,
    idx: usize,
    /// Human label for the call-stack view (e.g. `if (...) {true}`).
    pub label: String,
}

/// What a [`Stepper::step`] (or [`Stepper::choose`]) produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    pub kind: StopKind,
    /// Branch world-lines to choose from (when `kind == Branch`).
    pub branches: Vec<String>,
    /// Outcome label (when `kind == Terminated`).
    pub outcome: Option<String>,
    /// True when the step skipped an unmodeled construct (switch/loop/try).
    pub opaque: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopKind {
    /// Advanced over a plain statement; more to come.
    Stepped,
    /// Paused at a condition — call [`Stepper::choose`] with a branch index.
    Branch,
    /// Path ended (return / throw / process-terminating call).
    Terminated,
    /// Ran off the end of the method without an explicit return.
    FallThrough,
}

impl Stop {
    fn stepped(opaque: bool) -> Self {
        Self {
            kind: StopKind::Stepped,
            branches: Vec::new(),
            outcome: None,
            opaque,
        }
    }
    fn terminated(outcome: String) -> Self {
        Self {
            kind: StopKind::Terminated,
            branches: Vec::new(),
            outcome: Some(outcome),
            opaque: false,
        }
    }
    fn fall_through() -> Self {
        Self {
            kind: StopKind::FallThrough,
            branches: Vec::new(),
            outcome: None,
            opaque: false,
        }
    }
}

/// A pending world-line at a paused branch: the state to adopt and the body to
/// enter if this option is chosen.
#[derive(Clone)]
struct Pending<'a> {
    label: String,
    state: SymState,
    body: Option<Vec<tree_sitter::Node<'a>>>,
}

#[derive(Clone)]
struct Snapshot<'a> {
    stack: Vec<Frame<'a>>,
    state: SymState,
    atoms: AtomTable,
    finished: Option<String>,
}

pub struct Stepper<'a> {
    src: &'a [u8],
    atoms: AtomTable,
    terminal_calls: Vec<String>,
    stack: Vec<Frame<'a>>,
    state: SymState,
    finished: Option<String>,
    pending: Vec<Pending<'a>>,
    history: Vec<Snapshot<'a>>,
}

impl<'a> Stepper<'a> {
    /// Start stepping at a method `body` block node.
    pub fn start(
        body: tree_sitter::Node<'a>,
        src: &'a [u8],
        terminal_calls: Vec<String>,
        max_atoms: usize,
    ) -> Self {
        Self {
            src,
            atoms: AtomTable::new(max_atoms),
            terminal_calls,
            stack: vec![Frame {
                stmts: sym::named_children(body),
                idx: 0,
                label: "method".to_string(),
            }],
            state: SymState::default(),
            finished: None,
            pending: Vec::new(),
            history: Vec::new(),
        }
    }

    pub fn state(&self) -> &SymState {
        &self.state
    }

    pub fn atoms(&self) -> &AtomTable {
        &self.atoms
    }

    /// Condition atoms decided so far, as readable `name = value` pairs — the
    /// "Context Rules" of the current world line.
    pub fn context_rules(&self) -> Vec<String> {
        self.state
            .conds
            .iter()
            .map(|&(atom, value)| format!("{} == {}", self.atoms.name(atom), value))
            .collect()
    }

    /// Call-stack frame labels, outermost first.
    pub fn call_stack(&self) -> Vec<&str> {
        self.stack.iter().map(|f| f.label.as_str()).collect()
    }

    /// The statement node about to execute, for source highlighting.
    pub fn current(&self) -> Option<tree_sitter::Node<'a>> {
        let frame = self.stack.last()?;
        frame.stmts.get(frame.idx).copied()
    }

    pub fn is_finished(&self) -> bool {
        self.finished.is_some() || (self.stack.is_empty() && self.pending.is_empty())
    }

    pub fn is_paused_at_branch(&self) -> bool {
        !self.pending.is_empty()
    }

    /// The branch world-lines currently on offer (empty unless paused).
    /// Read-only — does not advance.
    pub fn peek_branches(&self) -> Vec<String> {
        self.pending.iter().map(|p| p.label.clone()).collect()
    }

    fn snapshot(&mut self) {
        self.history.push(Snapshot {
            stack: self.stack.clone(),
            state: self.state.clone(),
            atoms: self.atoms.clone(),
            finished: self.finished.clone(),
        });
    }

    /// Restore the state from before the last `step`/`choose`. Returns false
    /// if there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        match self.history.pop() {
            Some(snap) => {
                self.stack = snap.stack;
                self.state = snap.state;
                self.atoms = snap.atoms;
                self.finished = snap.finished;
                self.pending.clear();
                true
            }
            None => false,
        }
    }

    /// Advance one statement. At an `if` this pauses and returns
    /// [`StopKind::Branch`]; call [`Stepper::choose`] next.
    pub fn step(&mut self) -> Result<Stop, SymError> {
        if let Some(outcome) = &self.finished {
            return Ok(Stop::terminated(outcome.clone()));
        }
        if !self.pending.is_empty() {
            return Ok(self.branch_stop());
        }
        self.snapshot();

        // Pop exhausted frames (returning out of blocks/branches).
        while let Some(frame) = self.stack.last() {
            if frame.idx >= frame.stmts.len() {
                self.stack.pop();
            } else {
                break;
            }
        }
        let Some(frame) = self.stack.last_mut() else {
            return Ok(Stop::fall_through());
        };
        let stmt = frame.stmts[frame.idx];

        match stmt.kind() {
            "return_statement" => {
                let outcome = self.return_outcome(stmt);
                self.finished = Some(outcome.clone());
                self.stack.clear();
                Ok(Stop::terminated(outcome))
            }
            "throw_statement" => {
                let expr = sym::named_children(stmt)
                    .first()
                    .map(|n| sym::normalize(&sym::text(*n, self.src)))
                    .unwrap_or_default();
                let outcome = format!("throw {expr}");
                self.finished = Some(outcome.clone());
                self.stack.clear();
                Ok(Stop::terminated(outcome))
            }
            "local_variable_declaration" => {
                for declarator in sym::named_children(stmt) {
                    if declarator.kind() != "variable_declarator" {
                        continue;
                    }
                    if let (Some(_name), Some(value)) = (
                        declarator.child_by_field_name("name"),
                        declarator.child_by_field_name("value"),
                    ) {
                        // record_write on the synthetic `name = value` is awkward;
                        // mirror the declarator directly into writes.
                        let name = sym::normalize(&sym::text(
                            declarator.child_by_field_name("name").unwrap(),
                            self.src,
                        ));
                        let val = sym::normalize(&sym::text(value, self.src));
                        self.state.writes.insert(name, val);
                    }
                }
                self.advance();
                Ok(Stop::stepped(false))
            }
            "expression_statement" => {
                if let Some(expr) = sym::named_children(stmt).first() {
                    if expr.kind() == "method_invocation"
                        && sym::is_terminal_call(*expr, self.src, &self.terminal_calls)
                    {
                        let outcome = sym::normalize(&sym::text(*expr, self.src));
                        self.finished = Some(outcome.clone());
                        self.stack.clear();
                        return Ok(Stop::terminated(outcome));
                    }
                    sym::record_write(*expr, self.src, &mut self.state);
                }
                self.advance();
                Ok(Stop::stepped(false))
            }
            "block" => {
                let inner = sym::named_children(stmt);
                self.advance();
                self.stack.push(Frame {
                    stmts: inner,
                    idx: 0,
                    label: "block".to_string(),
                });
                Ok(Stop::stepped(false))
            }
            "if_statement" => self.enter_if(stmt),
            // Unmodeled in v1: step over, flag opaque, don't fabricate effects.
            _ => {
                self.advance();
                Ok(Stop::stepped(true))
            }
        }
    }

    /// Take world-line `option` at a paused branch and resume.
    pub fn choose(&mut self, option: usize) -> Result<Stop, SymError> {
        if self.pending.is_empty() {
            return self.step();
        }
        if option >= self.pending.len() {
            return Err(SymError::Malformed("branch option out of range"));
        }
        // The snapshot for undo was already taken when the branch was reached;
        // choosing is part of that same step.
        let chosen = self.pending.remove(option);
        self.pending.clear();
        self.state = chosen.state;
        if let Some(body) = chosen.body {
            self.stack.push(Frame {
                stmts: body,
                idx: 0,
                label: chosen.label,
            });
        }
        Ok(Stop::stepped(false))
    }

    /// Pop the current frame (finish the enclosing block/branch and resume the
    /// parent after it). No-op at the top frame.
    pub fn step_out(&mut self) -> Result<Stop, SymError> {
        if self.finished.is_some() || self.pending.is_empty() && self.stack.len() <= 1 {
            return self.step();
        }
        self.snapshot();
        self.pending.clear();
        if self.stack.len() > 1 {
            self.stack.pop();
        }
        Ok(Stop::stepped(false))
    }

    fn enter_if(&mut self, stmt: tree_sitter::Node<'a>) -> Result<Stop, SymError> {
        let cond = stmt
            .child_by_field_name("condition")
            .ok_or(SymError::Malformed("if without condition"))?;
        let cond = strip_parens(cond);
        let then_branch = stmt.child_by_field_name("consequence");
        let else_branch = stmt.child_by_field_name("alternative");

        // Advance past the if first, so each chosen branch resumes after it.
        self.advance();

        let worlds = {
            let mut ev = Evaluator {
                src: self.src,
                atoms: &mut self.atoms,
            };
            ev.eval(cond, self.state.clone())?
        };

        let cond_text = sym::normalize(&sym::text(cond, self.src));
        self.pending = worlds
            .into_iter()
            .map(|(state, value)| {
                let branch = if value { then_branch } else { else_branch };
                Pending {
                    label: format!("{cond_text} {{{value}}}"),
                    state,
                    body: branch.map(statements_of),
                }
            })
            .collect();

        // A bodyless else (the false world of an if without else) just
        // continues; collapse it so the user isn't asked a no-op question only
        // when there is genuinely one world.
        Ok(self.branch_stop())
    }

    fn branch_stop(&self) -> Stop {
        Stop {
            kind: StopKind::Branch,
            branches: self.pending.iter().map(|p| p.label.clone()).collect(),
            outcome: None,
            opaque: false,
        }
    }

    fn advance(&mut self) {
        if let Some(frame) = self.stack.last_mut() {
            frame.idx += 1;
        }
    }

    /// `return x` reports the value last assigned to `x` on this path.
    fn return_outcome(&self, stmt: tree_sitter::Node) -> String {
        match sym::named_children(stmt).first() {
            Some(node) => {
                let expr = sym::normalize(&sym::text(*node, self.src));
                let value = if node.kind() == "identifier" {
                    self.state.writes.get(&expr).cloned().unwrap_or(expr)
                } else {
                    expr
                };
                format!("return {value}")
            }
            None => "return".to_string(),
        }
    }
}

fn strip_parens(node: tree_sitter::Node) -> tree_sitter::Node {
    if node.kind() == "parenthesized_expression" {
        sym::named_children(node).first().copied().unwrap_or(node)
    } else {
        node
    }
}

fn statements_of(node: tree_sitter::Node) -> Vec<tree_sitter::Node> {
    if node.kind() == "block" {
        sym::named_children(node)
    } else {
        vec![node]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> tree_sitter::Tree {
        let mut p = tree_sitter::Parser::new();
        p.set_language(&tree_sitter_java::LANGUAGE.into()).unwrap();
        p.parse(src, None).unwrap()
    }

    fn method_body<'a>(tree: &'a tree_sitter::Tree, src: &[u8]) -> tree_sitter::Node<'a> {
        fn find<'a>(n: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
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
        let _ = src;
        find(tree.root_node()).unwrap()
    }

    fn stepper_for<'a>(tree: &'a tree_sitter::Tree, src: &'a [u8]) -> Stepper<'a> {
        Stepper::start(
            method_body(tree, src),
            src,
            vec!["System.exit".to_string()],
            64,
        )
    }

    #[test]
    fn linear_assignments_then_return() {
        let src = "class A { int m() { int x = 5; x = 7; return x; } }";
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());

        assert_eq!(s.step().unwrap().kind, StopKind::Stepped); // int x = 5
        assert_eq!(s.state().writes.get("x"), Some(&"5".to_string()));
        assert_eq!(s.step().unwrap().kind, StopKind::Stepped); // x = 7
        assert_eq!(s.state().writes.get("x"), Some(&"7".to_string()));
        let stop = s.step().unwrap(); // return x
        assert_eq!(stop.kind, StopKind::Terminated);
        assert_eq!(stop.outcome.as_deref(), Some("return 7"));
        assert!(s.is_finished());
    }

    #[test]
    fn if_pauses_and_choose_takes_a_world() {
        let src = r#"class A { String m(boolean a) {
            if (a) { return "yes"; }
            return "no";
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());

        let stop = s.step().unwrap();
        assert_eq!(stop.kind, StopKind::Branch);
        assert_eq!(stop.branches.len(), 2);
        assert!(s.is_paused_at_branch());

        // pick the world where `a` is true
        let true_idx = stop
            .branches
            .iter()
            .position(|b| b.contains("{true}"))
            .unwrap();
        let after = s.choose(true_idx).unwrap();
        assert_eq!(after.kind, StopKind::Stepped);
        assert_eq!(s.context_rules(), vec!["a == true"]);

        let stop = s.step().unwrap(); // return "yes"
        assert_eq!(stop.outcome.as_deref(), Some("return \"yes\""));
    }

    #[test]
    fn false_world_falls_through_to_after_if() {
        let src = r#"class A { String m(boolean a) {
            if (a) { return "yes"; }
            return "no";
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());

        let stop = s.step().unwrap();
        let false_idx = stop
            .branches
            .iter()
            .position(|b| b.contains("{false}"))
            .unwrap();
        s.choose(false_idx).unwrap(); // a == false, no body to enter
        let stop = s.step().unwrap(); // return "no"
        assert_eq!(stop.outcome.as_deref(), Some("return \"no\""));
        assert_eq!(s.context_rules(), vec!["a == false"]);
    }

    #[test]
    fn undo_restores_prior_state() {
        let src = "class A { int m() { int x = 1; x = 2; return x; } }";
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());

        s.step().unwrap(); // x = 1
        s.step().unwrap(); // x = 2
        assert_eq!(s.state().writes.get("x"), Some(&"2".to_string()));
        assert!(s.undo()); // back to x = 1
        assert_eq!(s.state().writes.get("x"), Some(&"1".to_string()));
        // re-step forward differently is possible; here just confirm progress
        let stop = s.step().unwrap();
        assert_eq!(s.state().writes.get("x"), Some(&"2".to_string()));
        assert_eq!(stop.kind, StopKind::Stepped);
    }

    #[test]
    fn terminal_call_ends_the_path() {
        let src = "class A { void m(boolean a) { if (a) { System.exit(1); } doRest(); } }";
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());
        let stop = s.step().unwrap();
        let t = stop
            .branches
            .iter()
            .position(|b| b.contains("{true}"))
            .unwrap();
        s.choose(t).unwrap();
        let stop = s.step().unwrap();
        assert_eq!(stop.kind, StopKind::Terminated);
        assert_eq!(stop.outcome.as_deref(), Some("System.exit(1)"));
    }

    #[test]
    fn switch_is_opaque_in_v1() {
        let src = r#"class A { int m(int x) {
            switch (x) { case 1: return 1; default: return 0; }
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());
        let stop = s.step().unwrap();
        assert!(stop.opaque);
        assert_eq!(stop.kind, StopKind::Stepped);
    }

    #[test]
    fn nested_blocks_pop_correctly() {
        let src = r#"class A { int m(boolean a) {
            if (a) { { int y = 1; } return 1; }
            return 0;
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());
        let stop = s.step().unwrap();
        let t = stop
            .branches
            .iter()
            .position(|b| b.contains("{true}"))
            .unwrap();
        s.choose(t).unwrap(); // enter the if-true body
                              // step through inner block { int y = 1; }
        let mut guard = 0;
        loop {
            let stop = s.step().unwrap();
            guard += 1;
            assert!(guard < 20, "did not terminate");
            if stop.kind == StopKind::Terminated {
                assert_eq!(stop.outcome.as_deref(), Some("return 1"));
                break;
            }
        }
    }
}
