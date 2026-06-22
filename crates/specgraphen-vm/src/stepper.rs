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
//! Scope: models `if`/`else`, blocks, sequential statements, assignments,
//! terminal calls, loops, and clean `switch`. Loops are a choice — enter the
//! body for one symbolic iteration, or skip it (zero iterations); both are
//! sound world-lines (N>1 not enumerated). A clean `switch` (every group ends
//! in break/return/throw, or arrow rules) becomes one world-line per case with
//! sequential-exclusivity atoms, sharing [`sym::parse_switch`] with the batch
//! extractor. Wrapper constructs are entered, not skipped: `try` steps the
//! no-exception happy path (body then finally; catch worlds aren't enumerated),
//! and `synchronized`/labeled statements step into their body — so logic buried
//! inside a method-wide `try` (the legacy norm) is reachable. Only
//! fall-through/conditional-break `switch` remains opaque (flagged via
//! [`Stop::opaque`]); if such a skipped construct can itself return/throw, the
//! path is marked [`Stepper::is_incomplete`] so its outcome isn't trusted.
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
    incomplete: bool,
}

#[derive(Clone)]
pub struct Stepper<'a> {
    src: &'a [u8],
    atoms: AtomTable,
    terminal_calls: Vec<String>,
    stack: Vec<Frame<'a>>,
    state: SymState,
    finished: Option<String>,
    pending: Vec<Pending<'a>>,
    history: Vec<Snapshot<'a>>,
    /// True once this path stepped over an unmodeled construct that can itself
    /// terminate the method (a `switch`/`try` containing return/throw): the
    /// reported outcome is then not trustworthy as the method's behavior.
    incomplete: bool,
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
            incomplete: false,
        }
    }

    /// True if this path stepped over a construct that could itself exit the
    /// method (an unmodeled `switch`/`try` with a return/throw inside), so a
    /// reported terminal outcome may not be the real one.
    pub fn is_incomplete(&self) -> bool {
        self.incomplete
    }

    pub fn state(&self) -> &SymState {
        &self.state
    }

    /// Seed an initial local binding before stepping begins — e.g. a callee
    /// parameter bound to the caller's actual-argument value, for cross-method
    /// step-into. At start there are no atoms yet, so this only records the
    /// write; a later condition on the parameter can then settle from it (the
    /// same `resolved_by_write` precision used for `x = "A"; if (x.equals("A"))`).
    pub fn seed_local(&mut self, name: &str, value: &str) {
        sym::bind_local(name, value, &self.atoms, &mut self.state);
    }

    pub fn atoms(&self) -> &AtomTable {
        &self.atoms
    }

    /// The outcome string of a finished path (`return …`, `throw …`, a
    /// terminal call), if this stepper has terminated. `None` while still
    /// running or on fall-through.
    pub fn finished_outcome(&self) -> Option<&str> {
        self.finished.as_deref()
    }

    /// Drop the undo history. Used by the all-paths enumerator, which clones a
    /// stepper at every branch and never undoes — keeping each clone's history
    /// would make DFS memory grow with depth for no benefit.
    pub fn forget_history(&mut self) {
        self.history.clear();
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
            incomplete: self.incomplete,
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
                self.incomplete = snap.incomplete;
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
                        sym::bind_local(&name, &val, &self.atoms, &mut self.state);
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
                    sym::record_write(*expr, self.src, &self.atoms, &mut self.state);
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
            // Loops model two sound world-lines: enter the body for one
            // symbolic iteration, or skip it (zero iterations). N>1 iterations
            // are not enumerated — interactive inspection of "what happens per
            // iteration" is the goal, not full unrolling.
            "while_statement" | "for_statement" | "enhanced_for_statement" => {
                self.enter_loop(stmt, true)
            }
            // do-while runs its body at least once: no zero-iteration world.
            "do_statement" => self.enter_loop(stmt, false),
            "switch_expression" | "switch_statement" => self.enter_switch(stmt),
            // Wrapper constructs whose body holds the real logic: step into it
            // rather than skipping. `try` models the no-exception happy path —
            // body then finally; catch (exception worlds) is not enumerated.
            "try_statement" | "try_with_resources_statement" => self.enter_try(stmt),
            "synchronized_statement" => self.enter_wrapper(stmt),
            "labeled_statement" => self.enter_labeled(stmt),
            // Still unmodeled: step over. If such a construct can itself
            // terminate the method, this path's reported outcome is unreliable.
            _ => {
                if contains_exit(stmt) {
                    self.incomplete = true;
                }
                self.advance();
                Ok(Stop::stepped(true))
            }
        }
    }

    /// Present a loop as a choice: enter the body once, or (when `skippable`)
    /// skip it entirely. Both resume after the loop — one symbolic iteration,
    /// not a full unroll.
    fn enter_loop(
        &mut self,
        stmt: tree_sitter::Node<'a>,
        skippable: bool,
    ) -> Result<Stop, SymError> {
        self.advance(); // past the loop; every world resumes after it
        let body = stmt.child_by_field_name("body").map(statements_of);

        // Entering the body binds a `for (T v : …)` loop variable to a fresh
        // element, so any condition atom pinned on `v` from an outer scope is
        // no longer valid inside the body.
        let mut entered = self.state.clone();
        if stmt.kind() == "enhanced_for_statement" {
            if let Some(name) = stmt.child_by_field_name("name") {
                let var = sym::normalize(&sym::text(name, self.src));
                sym::invalidate(&mut entered, &self.atoms, &var);
            }
        }

        if !skippable {
            // do-while: body always runs once; no branch needed.
            self.state = entered;
            if let Some(body) = body {
                self.stack.push(Frame {
                    stmts: body,
                    idx: 0,
                    label: "do-while body (1 iteration)".to_string(),
                });
            }
            return Ok(Stop::stepped(false));
        }

        self.pending = vec![
            Pending {
                label: "loop body (1 iteration)".to_string(),
                state: entered,
                body,
            },
            Pending {
                label: "skip loop (0 iterations)".to_string(),
                state: self.state.clone(),
                body: None,
            },
        ];
        Ok(self.branch_stop())
    }

    /// Present a clean `switch` as one world-line per case (plus default /
    /// no-match), with the same sequential-exclusivity atoms the batch
    /// extractor uses. Falls back to an opaque step when the switch can't be
    /// modeled faithfully (fall-through / conditional break).
    fn enter_switch(&mut self, stmt: tree_sitter::Node<'a>) -> Result<Stop, SymError> {
        let Some(model) = sym::parse_switch(stmt, self.src) else {
            if contains_exit(stmt) {
                self.incomplete = true;
            }
            self.advance();
            return Ok(Stop::stepped(true));
        };
        self.advance(); // past the switch; every world resumes after it

        let subject = &model.subject;
        let default_idx = model.groups.iter().position(|g| g.is_default);
        let mut pending = Vec::new();
        let mut seen: Vec<String> = Vec::new();

        for group in &model.groups {
            for value in &group.values {
                let mut branch = self.state.clone();
                let feasible = {
                    let mut ev = Evaluator {
                        src: self.src,
                        atoms: &mut self.atoms,
                    };
                    let mut ok = true;
                    for earlier in &seen {
                        if !ev.assign_atom(
                            &format!("{subject} == {earlier}"),
                            false,
                            &mut branch,
                        )? {
                            ok = false;
                            break;
                        }
                    }
                    if ok {
                        ok = ev.assign_atom(&format!("{subject} == {value}"), true, &mut branch)?;
                    }
                    ok
                };
                if feasible {
                    pending.push(Pending {
                        label: format!("{subject} == {value}"),
                        state: branch,
                        body: Some(group.body.clone()),
                    });
                }
                seen.push(value.clone());
            }
        }

        // The all-cases-false world: the default group, or skip past the switch.
        let mut fallback = self.state.clone();
        let feasible = {
            let mut ev = Evaluator {
                src: self.src,
                atoms: &mut self.atoms,
            };
            let mut ok = true;
            for value in &seen {
                if !ev.assign_atom(&format!("{subject} == {value}"), false, &mut fallback)? {
                    ok = false;
                    break;
                }
            }
            ok
        };
        if feasible {
            match default_idx {
                Some(gi) => pending.push(Pending {
                    label: "default (no case matches)".to_string(),
                    state: fallback,
                    body: Some(model.groups[gi].body.clone()),
                }),
                None => pending.push(Pending {
                    label: "no case matches (skip)".to_string(),
                    state: fallback,
                    body: None,
                }),
            }
        }

        self.pending = pending;
        Ok(self.settle())
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

    /// `try { BODY } catch ... finally { FIN }` — step the no-exception world:
    /// run BODY, then FIN, then continue after the try. Catch clauses
    /// (exception worlds) are not enumerated in v1. Resources of a
    /// try-with-resources are treated as opaque (not modeled).
    fn enter_try(&mut self, stmt: tree_sitter::Node<'a>) -> Result<Stop, SymError> {
        self.advance(); // past the try; everything resumes after it

        let mut cursor = stmt.walk();
        let children: Vec<tree_sitter::Node> = stmt.children(&mut cursor).collect();

        // finally runs after the body — push it first so it executes second.
        if let Some(fin) = children.iter().find(|c| c.kind() == "finally_clause") {
            if let Some(block) = block_child(*fin) {
                self.stack.push(Frame {
                    stmts: sym::named_children(block),
                    idx: 0,
                    label: "finally".to_string(),
                });
            }
        }
        if let Some(body) = stmt
            .child_by_field_name("body")
            .or_else(|| block_child(stmt))
        {
            self.stack.push(Frame {
                stmts: sym::named_children(body),
                idx: 0,
                label: "try body".to_string(),
            });
        }
        Ok(Stop::stepped(false))
    }

    /// `synchronized (x) { BODY }` — the lock is irrelevant to symbolic logic;
    /// step into the body.
    fn enter_wrapper(&mut self, stmt: tree_sitter::Node<'a>) -> Result<Stop, SymError> {
        self.advance();
        if let Some(body) = stmt
            .child_by_field_name("body")
            .or_else(|| block_child(stmt))
        {
            self.stack.push(Frame {
                stmts: sym::named_children(body),
                idx: 0,
                label: "synchronized".to_string(),
            });
        }
        Ok(Stop::stepped(false))
    }

    /// `label: stmt` — step into the labeled statement.
    fn enter_labeled(&mut self, stmt: tree_sitter::Node<'a>) -> Result<Stop, SymError> {
        self.advance();
        // The inner statement is the last named child (after the label id).
        if let Some(inner) = sym::named_children(stmt).into_iter().next_back() {
            self.stack.push(Frame {
                stmts: statements_of(inner),
                idx: 0,
                label: "labeled".to_string(),
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
        // A short-circuited `A && B` yields two *false* worlds (A=false; and
        // A=true,B=false) that would otherwise share an identical `{false}`
        // label. When a boolean value has more than one world, append the
        // atoms each world newly pinned so they're distinguishable; simple
        // single-world cases keep the clean `cond {value}` label.
        let base_len = self.state.conds.len();
        let true_count = worlds.iter().filter(|(_, v)| *v).count();
        let false_count = worlds.len() - true_count;
        let atoms = &self.atoms;
        let pending: Vec<Pending> = worlds
            .into_iter()
            .map(|(state, value)| {
                let branch = if value { then_branch } else { else_branch };
                let ambiguous = if value { true_count } else { false_count } > 1;
                let label = if ambiguous {
                    let pins: Vec<String> = state
                        .conds
                        .get(base_len..)
                        .unwrap_or(&[])
                        .iter()
                        .map(|&(id, v)| format!("{} == {}", atoms.name(id), v))
                        .collect();
                    // ⟨ … ∧ … ⟩ delimiters: a method-arg `, ` inside a pin
                    // won't be confused for a pin separator by the UI parser.
                    format!("{cond_text} {{{value}}}  ⟨{}⟩", pins.join(" ∧ "))
                } else {
                    format!("{cond_text} {{{value}}}")
                };
                Pending {
                    label,
                    state,
                    body: branch.map(statements_of),
                }
            })
            .collect();
        self.pending = pending;

        // A bodyless else (the false world of an if without else) just
        // continues; collapse it so the user isn't asked a no-op question only
        // when there is genuinely one world.
        Ok(self.settle())
    }

    fn branch_stop(&self) -> Stop {
        Stop {
            kind: StopKind::Branch,
            branches: self.pending.iter().map(|p| p.label.clone()).collect(),
            outcome: None,
            opaque: false,
        }
    }

    /// Resolve `self.pending`: with one feasible world there's no choice to
    /// make, so take it directly (a pinned/feasibility-collapsed condition
    /// shouldn't present a fake single-option branch); with several, pause.
    fn settle(&mut self) -> Stop {
        match self.pending.len() {
            0 => Stop::stepped(false),
            1 => {
                let only = self.pending.remove(0);
                self.state = only.state;
                if let Some(body) = only.body {
                    self.stack.push(Frame {
                        stmts: body,
                        idx: 0,
                        label: only.label,
                    });
                }
                Stop::stepped(false)
            }
            _ => self.branch_stop(),
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

/// The first direct `block` child of a node (for constructs whose body block
/// isn't exposed under a field name).
fn block_child(node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();
    children.into_iter().find(|c| c.kind() == "block")
}

/// Does this subtree contain a `return`/`throw` that would exit the method?
fn contains_exit(node: tree_sitter::Node) -> bool {
    if matches!(node.kind(), "return_statement" | "throw_statement") {
        return true;
    }
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();
    children.into_iter().any(contains_exit)
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

    /// A literal assigned *before* any condition references the variable still
    /// settles a later test — the atom is unknown at assignment time (so the
    /// re-pin in `record_write` can't fire), but eval consults the recorded
    /// write. `flag = true; if (!flag)` must not fork into a world where
    /// `flag` is false (which would contradict the variable table).
    #[test]
    fn boolean_literal_before_first_use_settles() {
        let src = r#"class A { int m() {
            boolean flag = false;
            flag = true;
            if (!flag) { return 1; }
            return 0;
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());
        // boolean flag = false; / flag = true; / (if !flag settles, no branch)
        for _ in 0..3 {
            assert_eq!(s.step().unwrap().kind, StopKind::Stepped);
        }
        let stop = s.step().unwrap();
        assert_eq!(stop.kind, StopKind::Terminated);
        assert_eq!(stop.outcome.as_deref(), Some("return 0"));
    }

    /// Same precision for equality-against-constant: a literal write to the
    /// subject decides `x.equals(c)` without forking.
    #[test]
    fn equals_constant_after_literal_write_settles() {
        let src = r#"class A { int m() {
            String x = "A";
            if (x.equals("A")) { return 1; }
            return 0;
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());
        // String x = "A"; / if (x.equals("A")) settles true and enters body
        for _ in 0..2 {
            assert_eq!(s.step().unwrap().kind, StopKind::Stepped);
        }
        let stop = s.step().unwrap(); // return 1
        assert_eq!(stop.kind, StopKind::Terminated);
        assert_eq!(stop.outcome.as_deref(), Some("return 1"));
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
    fn while_loop_offers_enter_or_skip() {
        let src = r#"class A { int m(java.util.Iterator<String> it) {
            int n = 0;
            while (it.hasNext()) { n = n + 1; process(it.next()); }
            return n;
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());
        s.step().unwrap(); // int n = 0
        let stop = s.step().unwrap(); // while → branch
        assert_eq!(stop.kind, StopKind::Branch);
        assert_eq!(stop.branches.len(), 2);
        assert!(stop.branches.iter().any(|b| b.contains("1 iteration")));
        assert!(stop.branches.iter().any(|b| b.contains("0 iterations")));
    }

    #[test]
    fn entering_loop_body_steps_through_one_iteration() {
        let src = r#"class A { String m(java.util.Iterator<String> it) {
            while (it.hasNext()) { return "in-loop"; }
            return "after";
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());
        let stop = s.step().unwrap(); // while → branch
        let enter = stop
            .branches
            .iter()
            .position(|b| b.contains("1 iteration"))
            .unwrap();
        s.choose(enter).unwrap();
        let stop = s.step().unwrap(); // return "in-loop"
        assert_eq!(stop.outcome.as_deref(), Some("return \"in-loop\""));
    }

    #[test]
    fn skipping_loop_resumes_after_it() {
        let src = r#"class A { String m(java.util.Iterator<String> it) {
            while (it.hasNext()) { return "in-loop"; }
            return "after";
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());
        let stop = s.step().unwrap();
        let skip = stop
            .branches
            .iter()
            .position(|b| b.contains("0 iterations"))
            .unwrap();
        s.choose(skip).unwrap();
        let stop = s.step().unwrap(); // return "after"
        assert_eq!(stop.outcome.as_deref(), Some("return \"after\""));
        assert!(
            !s.is_incomplete(),
            "skipping a loop is a sound 0-iteration world"
        );
    }

    #[test]
    fn do_while_body_runs_without_a_skip_choice() {
        let src = r#"class A { String m(java.util.Iterator<String> it) {
            do { return "body"; } while (it.hasNext());
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());
        let stop = s.step().unwrap(); // do → enters body directly (no branch)
        assert_eq!(stop.kind, StopKind::Stepped);
        let stop = s.step().unwrap(); // return "body"
        assert_eq!(stop.outcome.as_deref(), Some("return \"body\""));
    }

    #[test]
    fn clean_switch_offers_a_world_per_case() {
        let src = r#"class A { String m(String code) {
            switch (code) {
                case "A": return "alpha";
                case "B": return "beta";
                default: return "other";
            }
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());
        let stop = s.step().unwrap(); // switch → branch
        assert_eq!(stop.kind, StopKind::Branch);
        // two cases + default = 3 world-lines
        assert_eq!(stop.branches.len(), 3, "{:?}", stop.branches);
        let beta = stop
            .branches
            .iter()
            .position(|b| b.contains("\"B\""))
            .unwrap();
        s.choose(beta).unwrap();
        // sequential exclusivity: case "B" implies case "A" compared false
        assert!(s
            .context_rules()
            .iter()
            .any(|r| r.contains("\"A\" == false")));
        let stop = s.step().unwrap();
        assert_eq!(stop.outcome.as_deref(), Some("return \"beta\""));
        assert!(!s.is_incomplete());
    }

    #[test]
    fn try_body_is_entered_so_inner_logic_is_reachable() {
        // The real legacy shape: business logic (here a loop) lives inside a
        // try. Entering the try body must make that loop reachable.
        let src = r#"class A { String m(java.util.Iterator<String> it) {
            try {
                while (it.hasNext()) { return "in-loop"; }
            } finally {
                cleanup();
            }
            return "after";
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());
        let stop = s.step().unwrap(); // try → enter body (not opaque)
        assert!(!stop.opaque, "try body should be entered, not skipped");
        assert!(!s.is_incomplete());
        let stop = s.step().unwrap(); // while → loop branch
        assert_eq!(stop.kind, StopKind::Branch);
        let enter = stop
            .branches
            .iter()
            .position(|b| b.contains("1 iteration"))
            .unwrap();
        s.choose(enter).unwrap();
        let stop = s.step().unwrap(); // return "in-loop"
        assert_eq!(stop.outcome.as_deref(), Some("return \"in-loop\""));
    }

    #[test]
    fn try_finally_runs_after_body() {
        let src = r#"class A { void m() {
            try { this.a = 1; } finally { this.b = 2; }
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());
        s.step().unwrap(); // enter try → pushes finally then body
                           // step until we've executed both writes or run out
        let mut guard = 0;
        while !s.is_finished() && guard < 20 {
            s.step().unwrap();
            guard += 1;
        }
        assert_eq!(s.state().writes.get("this.a"), Some(&"1".to_string()));
        assert_eq!(s.state().writes.get("this.b"), Some(&"2".to_string()));
    }

    #[test]
    fn reassigned_flag_is_not_stale_pinned() {
        // A common legacy shape: a flag is tested, reassigned true in the same
        // region, then re-tested. The old behavior pinned the first test's
        // value and contradicted the assignment; now the literal re-pin makes
        // the re-test follow the assigned value.
        let src = r#"class A { String m() {
            if (!flag) {
                flag = true;
                if (flag) { return "on"; }
                return "unreachable";
            }
            return "off";
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());
        let stop = s.step().unwrap(); // if (!flag)
                                      // take the !flag == true world (flag pinned false)
        let t = stop
            .branches
            .iter()
            .position(|b| b.contains("{true}"))
            .unwrap();
        s.choose(t).unwrap();
        s.step().unwrap(); // flag = true → re-pins flag atom to true
                           // if (flag): flag now pinned true → no fork, enters the body
        let stop = s.step().unwrap();
        assert!(
            stop.kind != StopKind::Branch,
            "flag reassigned true; re-test must follow it, not the stale pin: {stop:?}"
        );
        let stop = s.step().unwrap();
        assert_eq!(stop.outcome.as_deref(), Some("return \"on\""));
    }

    #[test]
    fn reassigning_a_condition_variable_reforks() {
        // After `x = readNext()` (opaque value), a prior `x.equals("A")` pin is
        // dropped, so a later test of `x` re-forks rather than staying stuck.
        let src = r#"class A { String m(String x) {
            if (x.equals("A")) {
                x = readNext();
                if (x.equals("A")) { return "again"; }
                return "moved";
            }
            return "no";
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());
        let stop = s.step().unwrap(); // if x.equals("A")
        let t = stop
            .branches
            .iter()
            .position(|b| b.contains("{true}"))
            .unwrap();
        s.choose(t).unwrap();
        s.step().unwrap(); // x = readNext() → invalidates x.equals("A") pin
        let stop = s.step().unwrap(); // second if x.equals("A") → re-forks
        assert_eq!(
            stop.kind,
            StopKind::Branch,
            "reassigned x must let the re-test fork: {stop:?}"
        );
        assert_eq!(stop.branches.len(), 2);
    }

    #[test]
    fn reassign_inside_loop_body_reforks_equals() {
        // Outer test pins s.equals("X")=true; the re-scan loop body reassigns
        // s, so the inner re-test must re-fork (not follow the stale pin).
        let src = r#"class A { String m(String s, java.util.List<String> xs) {
            if (s.equals("X")) {
                for (String e : xs) {
                    s = e;
                    if (!s.equals("X")) { return "moved"; }
                }
                return "after";
            }
            return "other";
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());
        let stop = s.step().unwrap(); // if s.equals("X")
        let t = stop
            .branches
            .iter()
            .position(|b| b.contains("{true}"))
            .unwrap();
        s.choose(t).unwrap();
        let stop = s.step().unwrap(); // for-each → loop branch
        let enter = stop
            .branches
            .iter()
            .position(|b| b.contains("1 iteration"))
            .unwrap();
        s.choose(enter).unwrap();
        s.step().unwrap(); // s = e  → invalidates s.equals("X")
        let stop = s.step().unwrap(); // if (!s.equals("X")) → must re-fork
        assert_eq!(
            stop.kind,
            StopKind::Branch,
            "reassigned s must re-fork: {stop:?}"
        );
    }

    #[test]
    fn second_reassignment_identical_rhs_shadowed_var_reforks() {
        // Field "C" minimal repro: kk assigned in two separate loops, both with
        // identical RHS text (`o.trim()`), `o` a same-named shadowing loop var;
        // the 1st loop pins kk.equals("Z")==true into the PC; the 2nd loop's
        // reassignment must retract it so the re-test forks.
        let src = r#"class A { boolean m(java.util.List<String> a, java.util.List<String> b) {
            String kk = "";
            boolean found = false;
            for (String o : a) { kk = o.trim(); if (kk.equals("Z")) { found = true; } }
            for (String o : b) { kk = o.trim(); if (!kk.equals("Z")) { return false; } }
            return found;
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());

        let pick = |s: &mut Stepper, needle: &str| {
            let stop = s.step().unwrap();
            assert_eq!(stop.kind, StopKind::Branch, "expected a branch: {stop:?}");
            let i = stop
                .branches
                .iter()
                .position(|b| b.contains(needle))
                .unwrap_or_else(|| panic!("no branch matching {needle:?} in {:?}", stop.branches));
            s.choose(i).unwrap();
        };

        s.step().unwrap(); // String kk = ""
        s.step().unwrap(); // boolean found = false
        pick(&mut s, "1 iteration"); // enter loop a
        s.step().unwrap(); // kk = o.trim()
        pick(&mut s, "{true}"); // if (kk.equals("Z")) → true; pins kk.equals("Z")==true
        s.step().unwrap(); // found = true
                           // fall out of loop a, reach loop b
        pick(&mut s, "1 iteration"); // enter loop b
        s.step().unwrap(); // kk = o.trim()  → must retract kk.equals("Z")
        let stop = s.step().unwrap(); // if (!kk.equals("Z"))
        assert_eq!(
            stop.kind,
            StopKind::Branch,
            "2nd reassignment must retract the stale equals pin so the re-test forks: {stop:?}"
        );
    }

    #[test]
    fn reassign_after_closed_try_frame_reforks() {
        // Field "C" repro per analyst: pin is set inside a loop that lives in a
        // try{}finally{}; the try closes; a separate later loop reassigns the
        // same local. The reassignment must still retract the stale pin.
        let src = r#"class A { boolean m(String o0, java.util.List<String> list, boolean cond) {
            String kk = "";
            boolean found = false;
            try {
                while (cond) {
                    kk = o0.trim();
                    if (kk.equals("Z")) { found = true; }
                }
            } finally { cleanup(); }
            for (String o : list) {
                kk = o.trim();
                if (!kk.equals("Z")) { return false; }
            }
            return found;
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());

        let pick = |s: &mut Stepper, needle: &str| {
            let stop = s.step().unwrap();
            assert_eq!(stop.kind, StopKind::Branch, "expected branch: {stop:?}");
            let i = stop
                .branches
                .iter()
                .position(|b| b.contains(needle))
                .unwrap_or_else(|| panic!("no {needle:?} in {:?}", stop.branches));
            s.choose(i).unwrap();
        };

        // Drive to the pin, through the try/finally, into loop2, to the re-test.
        // Step until we either reach the second `if (!kk.equals("Z"))` (Branch)
        // or run to a terminal, choosing the pinning/iterating worlds en route.
        s.step().unwrap(); // String kk = ""
        s.step().unwrap(); // boolean found = false
        s.step().unwrap(); // try → enter (pushes finally, then try body)
        pick(&mut s, "1 iteration"); // while → enter body
        s.step().unwrap(); // kk = o0.trim()
        pick(&mut s, "{true}"); // if (kk.equals("Z")) → pin kk.equals("Z")==true
        s.step().unwrap(); // found = true
        s.step().unwrap(); // cleanup() (finally)
        pick(&mut s, "1 iteration"); // for-each list → enter body
        s.step().unwrap(); // kk = o.trim() → must retract kk.equals("Z")
        let stop = s.step().unwrap(); // if (!kk.equals("Z"))
        assert_eq!(
            stop.kind,
            StopKind::Branch,
            "reassignment after a closed try frame must retract the stale pin: {stop:?}"
        );
    }

    #[test]
    fn foreach_loop_variable_rebind_reforks_equals() {
        // The loop VARIABLE shares the name of an outer-pinned subject. Entering
        // the body rebinds it, so a pinned `code.equals("X")` from outside must
        // not force the in-body re-test (this was the residual "C" case).
        let src = r#"class A { String m(String code, java.util.List<String> codes) {
            if (code.equals("X")) {
                for (String code : codes) {
                    if (!code.equals("X")) { return "diff"; }
                }
            }
            return "done";
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());
        let stop = s.step().unwrap(); // if code.equals("X")
        let t = stop
            .branches
            .iter()
            .position(|b| b.contains("{true}"))
            .unwrap();
        s.choose(t).unwrap();
        let stop = s.step().unwrap(); // for-each → loop branch
        let enter = stop
            .branches
            .iter()
            .position(|b| b.contains("1 iteration"))
            .unwrap();
        s.choose(enter).unwrap(); // entering rebinds `code` → pin invalidated
        let stop = s.step().unwrap(); // if (!code.equals("X")) → must re-fork
        assert_eq!(
            stop.kind,
            StopKind::Branch,
            "loop-var `code` rebound; re-test must re-fork, not follow stale pin: {stop:?}"
        );
    }

    #[test]
    fn fallthrough_switch_with_exit_marks_incomplete() {
        // case 1 falls into case 2 (no break) → unmodelable → opaque; it
        // contains a return, so the post-switch outcome is untrusted.
        let src = r#"class A { String m(int x) {
            switch (x) { case 1: doOne(); case 2: return "two"; default: break; }
            return "after";
        } }"#;
        let tree = parse(src);
        let mut s = stepper_for(&tree, src.as_bytes());
        let stop = s.step().unwrap(); // switch → opaque (fall-through)
        assert!(stop.opaque);
        assert!(s.is_incomplete());
    }

    #[test]
    fn fallthrough_switch_stays_opaque() {
        // case 1 falls into case 2 (no break/return) — not faithfully modelable,
        // so it stays an opaque step rather than fabricating world-lines.
        let src = r#"class A { void m(int x) {
            switch (x) { case 1: doOne(); case 2: doTwo(); break; default: break; }
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
