//! Extracts decision tables from Java method bodies.
//!
//! Walks each method's `if`/`else` structure and enumerates execution paths.
//! Conditions are atomized by normalized expression text (same text = same
//! boolean variable); `&&` / `||` are decomposed with short-circuit path
//! semantics and `!` flips the branch value. A condition atom that reappears
//! on a path follows its already-assigned value, so contradictory paths
//! (`if (a) return; if (a) ...`) are never enumerated — this prevents
//! infeasible paths from being treated as don't-cares downstream.
//!
//! Outcomes are observable behavior, not just return text: variable writes
//! are tracked symbolically along each path, `return x` reports the value
//! last assigned to `x`, and writes to non-local targets (fields) are
//! appended to the outcome label. Paths that differ only in a field
//! mutation therefore form distinct outcome classes — without this,
//! assignment-driven methods would report their conditions as falsely dead.
//!
//! Clean `switch` statements (every group ending in break/return/throw, or
//! arrow rules) are modeled with else-if-chain semantics: entering case k
//! pins all earlier case values to false, so mutually exclusive cases never
//! produce infeasible don't-cares. Configurable terminal calls
//! (`System.exit` built in) end a path where they occur.
//!
//! Honesty over coverage: constructs this walker does not model (loops,
//! `try`, fall-through/conditional-break `switch`, …) mark the method
//! [`MethodDecision::incomplete`] when they can terminate the method
//! (contain `return`/`throw`), methods exceeding the atom/path caps are
//! skipped with a reason instead of producing a partial table, and side
//! effects of method calls (`list.add(...)` …) remain unmodeled.

use specgraphen_logic::{DecisionTable, Tri};

/// Atom cap mirrors the minimizer's variable limit.
const MAX_ATOMS: usize = specgraphen_logic::MAX_VARIABLES;
const MAX_PATHS: usize = 512;

/// Decision table extracted from one method.
#[derive(Debug)]
pub struct MethodDecision {
    /// `package.Class` (matches the lift FQN convention); empty for default package top level.
    pub class_fqn: String,
    pub method_name: String,
    pub start_line: u32,
    pub table: DecisionTable,
    /// True when the body contains unmodeled constructs (loop/switch/try …)
    /// that can terminate the method; the table omits those outcomes.
    pub incomplete: bool,
}

/// Identity of the method FQN this decision belongs to.
impl MethodDecision {
    pub fn fqn(&self) -> String {
        if self.class_fqn.is_empty() {
            self.method_name.clone()
        } else {
            format!("{}.{}", self.class_fqn, self.method_name)
        }
    }
}

#[derive(Debug, Default)]
pub struct DecisionExtraction {
    pub methods: Vec<MethodDecision>,
    /// Methods that could not be extracted: (method FQN, reason).
    pub skipped: Vec<(String, String)>,
}

pub struct DecisionExtractor {
    parser: tree_sitter::Parser,
    /// Callee names that never return (`System.exit` is built in); a path
    /// reaching one terminates there instead of flowing to an unreachable
    /// `return`. Entries match the callee (`abortOnError`) or its dotted
    /// form (`Util.abort`).
    terminal_calls: Vec<String>,
}

impl DecisionExtractor {
    pub fn new() -> anyhow::Result<Self> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&tree_sitter_java::LANGUAGE.into())?;
        Ok(Self {
            parser,
            terminal_calls: vec!["System.exit".to_string()],
        })
    }

    /// Register additional process-terminating helpers (legacy codebases
    /// often wrap `System.exit` in their own error-exit methods).
    pub fn with_terminal_calls(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.terminal_calls.extend(names);
        self
    }

    /// Extract decision tables for every branching method in `source`.
    /// Methods without any conditional are not reported.
    pub fn extract(&mut self, source: &str) -> anyhow::Result<DecisionExtraction> {
        let tree = self
            .parser
            .parse(source, None)
            .ok_or_else(|| anyhow::anyhow!("failed to parse source"))?;

        let mut result = DecisionExtraction::default();
        let mut class_stack: Vec<String> = Vec::new();
        let package = find_package(tree.root_node(), source.as_bytes());
        visit(
            tree.root_node(),
            source.as_bytes(),
            &package,
            &mut class_stack,
            &self.terminal_calls,
            &mut result,
        );
        Ok(result)
    }
}

fn find_package(root: tree_sitter::Node, src: &[u8]) -> Option<String> {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "package_declaration" {
            let mut inner = child.walk();
            for c in child.children(&mut inner) {
                if c.kind() == "scoped_identifier" || c.kind() == "identifier" {
                    return Some(text(c, src));
                }
            }
        }
    }
    None
}

fn visit(
    node: tree_sitter::Node,
    src: &[u8],
    package: &Option<String>,
    class_stack: &mut Vec<String>,
    terminal_calls: &[String],
    result: &mut DecisionExtraction,
) {
    match node.kind() {
        "class_declaration"
        | "interface_declaration"
        | "enum_declaration"
        | "record_declaration" => {
            let name = node
                .child_by_field_name("name")
                .map(|n| text(n, src))
                .unwrap_or_default();
            class_stack.push(name);
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                visit(child, src, package, class_stack, terminal_calls, result);
            }
            class_stack.pop();
        }
        "method_declaration" | "constructor_declaration" => {
            extract_method(node, src, package, class_stack, terminal_calls, result);
            // nested local classes inside method bodies are rare; skip
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                visit(child, src, package, class_stack, terminal_calls, result);
            }
        }
    }
}

fn extract_method(
    node: tree_sitter::Node,
    src: &[u8],
    package: &Option<String>,
    class_stack: &[String],
    terminal_calls: &[String],
    result: &mut DecisionExtraction,
) {
    let method_name = node
        .child_by_field_name("name")
        .map(|n| text(n, src))
        .unwrap_or_default();
    let class_fqn = {
        let mut parts: Vec<String> = Vec::new();
        if let Some(pkg) = package {
            parts.push(pkg.clone());
        }
        parts.extend(class_stack.iter().cloned());
        parts.join(".")
    };
    let fqn = if class_fqn.is_empty() {
        method_name.clone()
    } else {
        format!("{class_fqn}.{method_name}")
    };

    let Some(body) = node.child_by_field_name("body") else {
        return; // abstract / interface method
    };

    let mut walker = Walker {
        src,
        atoms: Vec::new(),
        locals: parameter_names(node, src),
        terminal_calls,
        paths: Vec::new(),
        incomplete: false,
    };

    let stmts = named_children(body);
    match walker.walk(&stmts, 0, State::default()) {
        Ok(fallthroughs) => {
            for state in fallthroughs {
                walker.paths.push((state, "(fall-through)".to_string()));
            }
        }
        Err(reason) => {
            result.skipped.push((fqn, reason));
            return;
        }
    }

    if walker.atoms.is_empty() || walker.paths.is_empty() {
        return; // no branching — nothing to compress
    }
    if walker.paths.len() > MAX_PATHS {
        result
            .skipped
            .push((fqn, format!("more than {MAX_PATHS} paths")));
        return;
    }

    // Observable effects per path (field writes + statement calls). Effects
    // shared by every path carry no discriminative information — subtract
    // them so outcome labels show only what the conditions actually change.
    let path_effects: Vec<Vec<String>> = walker
        .paths
        .iter()
        .map(|(state, _)| {
            state
                .writes
                .iter()
                .filter(|(target, _)| !walker.locals.contains(target.as_str()))
                .map(|(target, value)| format!("{target} = {value}"))
                .chain(state.calls.iter().cloned())
                .collect()
        })
        .collect();
    let common: std::collections::HashSet<&String> =
        path_effects
            .iter()
            .skip(1)
            .fold(path_effects[0].iter().collect(), |acc, effects| {
                let set: std::collections::HashSet<&String> = effects.iter().collect();
                acc.intersection(&set).copied().collect()
            });

    let mut table = DecisionTable::new(walker.atoms.clone());
    for ((state, core), effects) in walker.paths.iter().zip(&path_effects) {
        let distinct: Vec<&str> = effects
            .iter()
            .filter(|e| !common.contains(e))
            .map(String::as_str)
            .collect();
        let outcome = if distinct.is_empty() {
            core.clone()
        } else {
            format!("{core} {{{}}}", distinct.join(", "))
        };

        let mut inputs = vec![Tri::Any; walker.atoms.len()];
        for &(atom, value) in &state.conds {
            inputs[atom] = if value { Tri::True } else { Tri::False };
        }
        table
            .add_row(inputs, outcome)
            .expect("row arity matches atom count");
    }

    result.methods.push(MethodDecision {
        class_fqn,
        method_name,
        start_line: node.start_position().row as u32 + 1,
        table,
        incomplete: walker.incomplete,
    });
}

/// Per-path symbolic state: condition-atom assignments plus observable effects.
#[derive(Debug, Clone, Default)]
struct State {
    conds: Vec<(usize, bool)>,
    /// Normalized assignment target → last assigned value text along this path.
    writes: std::collections::BTreeMap<String, String>,
    /// Statement-level method calls along this path (`abortOnError()`,
    /// `sendNotification(...)` …) — in legacy code these ARE the outcome.
    calls: std::collections::BTreeSet<String>,
}

struct Walker<'a> {
    src: &'a [u8],
    atoms: Vec<String>,
    /// Names that cannot escape the method (parameters and declared locals);
    /// writes to them are invisible in outcomes unless returned.
    locals: std::collections::HashSet<String>,
    /// Callee names that terminate the process — a path ends there.
    terminal_calls: &'a [String],
    paths: Vec<(State, String)>,
    incomplete: bool,
}

impl Walker<'_> {
    /// Walk `stmts[idx..]`; terminated paths are recorded in `self.paths`,
    /// states that fall off the end are returned for the caller to continue.
    fn walk(
        &mut self,
        stmts: &[tree_sitter::Node],
        idx: usize,
        state: State,
    ) -> Result<Vec<State>, String> {
        if self.paths.len() > MAX_PATHS {
            return Err(format!("more than {MAX_PATHS} paths"));
        }
        let Some(&stmt) = stmts.get(idx) else {
            return Ok(vec![state]);
        };

        match stmt.kind() {
            "return_statement" => {
                let core = match named_children(stmt).first() {
                    Some(node) => {
                        let expr = normalize(&text(*node, self.src));
                        // `return x` after `x = <v>` reports the assigned value,
                        // so paths that differ only in the write stay distinct.
                        let value = if node.kind() == "identifier" {
                            state.writes.get(&expr).cloned().unwrap_or(expr)
                        } else {
                            expr
                        };
                        format!("return {value}")
                    }
                    None => "return".to_string(),
                };
                self.paths.push((state, core));
                Ok(Vec::new())
            }
            "throw_statement" => {
                let expr = named_children(stmt)
                    .first()
                    .map(|n| normalize(&text(*n, self.src)))
                    .unwrap_or_default();
                self.paths.push((state, format!("throw {expr}")));
                Ok(Vec::new())
            }
            "local_variable_declaration" => {
                let mut state = state;
                for declarator in named_children(stmt) {
                    if declarator.kind() != "variable_declarator" {
                        continue;
                    }
                    let Some(name) = declarator.child_by_field_name("name") else {
                        continue;
                    };
                    let name = normalize(&text(name, self.src));
                    self.locals.insert(name.clone());
                    if let Some(value) = declarator.child_by_field_name("value") {
                        state.writes.insert(name, normalize(&text(value, self.src)));
                    }
                }
                self.walk(stmts, idx + 1, state)
            }
            "expression_statement" => {
                let mut state = state;
                if let Some(expr) = named_children(stmt).first() {
                    if expr.kind() == "method_invocation" && self.is_terminal_call(*expr) {
                        // Process-terminating call: the path ends here; any
                        // code after it is unreachable on this path.
                        let call = normalize(&text(*expr, self.src));
                        self.paths.push((state, call));
                        return Ok(Vec::new());
                    }
                    self.record_write(*expr, &mut state);
                }
                self.walk(stmts, idx + 1, state)
            }
            "if_statement" => {
                let cond = stmt
                    .child_by_field_name("condition")
                    .ok_or("if without condition")?;
                let then_branch = stmt.child_by_field_name("consequence");
                let else_branch = stmt.child_by_field_name("alternative");

                let mut fallthroughs = Vec::new();
                for (branch_state, value) in self.eval(cond, state)? {
                    let branch = if value { then_branch } else { else_branch };
                    let branch_ends = match branch {
                        Some(b) => {
                            let branch_stmts = statements_of(b);
                            self.walk(&branch_stmts, 0, branch_state)?
                        }
                        None => vec![branch_state],
                    };
                    for end in branch_ends {
                        fallthroughs.extend(self.walk(stmts, idx + 1, end)?);
                    }
                }
                Ok(fallthroughs)
            }
            "block" => {
                let inner = named_children(stmt);
                let mut fallthroughs = Vec::new();
                for end in self.walk(&inner, 0, state)? {
                    fallthroughs.extend(self.walk(stmts, idx + 1, end)?);
                }
                Ok(fallthroughs)
            }
            "switch_expression" | "switch_statement" => {
                match self.model_switch(stmt, &state)? {
                    Some(exits) => {
                        let mut fallthroughs = Vec::new();
                        for end in exits {
                            fallthroughs.extend(self.walk(stmts, idx + 1, end)?);
                        }
                        Ok(fallthroughs)
                    }
                    // Unmodelable (fall-through / conditional break): same
                    // honesty rule as loops — don't fabricate outcomes.
                    None => {
                        if contains_exit(stmt) {
                            self.incomplete = true;
                        }
                        self.walk(stmts, idx + 1, state)
                    }
                }
            }
            // Constructs we do not model: if they can terminate the method,
            // the table is incomplete (we'd otherwise fabricate outcomes).
            "while_statement"
            | "for_statement"
            | "enhanced_for_statement"
            | "do_statement"
            | "try_statement"
            | "try_with_resources_statement"
            | "synchronized_statement"
            | "labeled_statement" => {
                if contains_exit(stmt) {
                    self.incomplete = true;
                }
                self.walk(stmts, idx + 1, state)
            }
            _ => self.walk(stmts, idx + 1, state),
        }
    }

    /// Does this invocation match a configured process-terminating callee?
    /// Matches the bare name (`abortOnError`) or `receiver.name` (`System.exit`).
    fn is_terminal_call(&self, invocation: tree_sitter::Node) -> bool {
        let Some(name) = invocation.child_by_field_name("name") else {
            return false;
        };
        let name = text(name, self.src);
        let dotted = invocation
            .child_by_field_name("object")
            .map(|o| format!("{}.{}", text(o, self.src), name));
        self.terminal_calls
            .iter()
            .any(|t| *t == name || Some(t.as_str()) == dotted.as_deref())
    }

    /// Record an assignment / increment as a symbolic write. Anything else
    /// (method calls etc.) is ignored — their side effects are not modeled.
    fn record_write(&mut self, expr: tree_sitter::Node, state: &mut State) {
        match expr.kind() {
            "assignment_expression" => {
                let (Some(left), Some(right)) = (
                    expr.child_by_field_name("left"),
                    expr.child_by_field_name("right"),
                ) else {
                    return;
                };
                let target = normalize(&text(left, self.src));
                let operator = expr
                    .child_by_field_name("operator")
                    .map(|n| text(n, self.src))
                    .unwrap_or_default();
                // Compound assignment (`+=` …) has no simple value; keep the
                // whole expression text — outcomes still distinguish the paths.
                let value = if operator == "=" {
                    normalize(&text(right, self.src))
                } else {
                    normalize(&text(expr, self.src))
                };
                state.writes.insert(target, value);
            }
            "update_expression" => {
                // i++ / --i: opaque value, but an observable write
                let whole = normalize(&text(expr, self.src));
                let target = whole.trim_matches(['+', '-', ' ']).to_string();
                if !target.is_empty() {
                    state.writes.insert(target, whole);
                }
            }
            "method_invocation" => {
                // Statement-level calls (error exits, state mutations) are
                // observable behavior — without them, branches that only call
                // out are reported as falsely dead. Logging is excluded: it is
                // not business behavior, and per-branch log messages would
                // otherwise explode the outcome alphabet and kill compression.
                let call = normalize(&text(expr, self.src));
                if !is_logging_call(&call) {
                    state.calls.insert(call);
                }
            }
            _ => {}
        }
    }

    /// Evaluate a condition with short-circuit path semantics.
    /// Returns every (state, truth-value) pair the condition can produce.
    fn eval(
        &mut self,
        node: tree_sitter::Node,
        state: State,
    ) -> Result<Vec<(State, bool)>, String> {
        match node.kind() {
            "parenthesized_expression" => {
                let inner = named_children(node);
                let inner = inner.first().ok_or("empty parentheses")?;
                self.eval(*inner, state)
            }
            "unary_expression" => {
                let op = node
                    .child_by_field_name("operator")
                    .map(|n| text(n, self.src));
                let operand = node
                    .child_by_field_name("operand")
                    .ok_or("unary without operand")?;
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

    /// `&&` (and=true): left=false short-circuits to false; left=true
    /// continues into the right operand. `||` is the mirror image.
    fn short_circuit(
        &mut self,
        node: tree_sitter::Node,
        state: State,
        and: bool,
    ) -> Result<Vec<(State, bool)>, String> {
        let left = node.child_by_field_name("left").ok_or("missing lhs")?;
        let right = node.child_by_field_name("right").ok_or("missing rhs")?;

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

    fn atom_id(&mut self, label: String) -> Result<usize, String> {
        match self.atoms.iter().position(|a| a == &label) {
            Some(id) => Ok(id),
            None => {
                if self.atoms.len() >= MAX_ATOMS {
                    return Err(format!("more than {MAX_ATOMS} distinct conditions"));
                }
                self.atoms.push(label);
                Ok(self.atoms.len() - 1)
            }
        }
    }

    /// Treat `node` as an atomic condition: same normalized text, same variable.
    /// A path that already assigned this atom follows that value (feasibility).
    fn atom(
        &mut self,
        node: tree_sitter::Node,
        state: State,
    ) -> Result<Vec<(State, bool)>, String> {
        let label = normalize(&text(node, self.src));
        let id = self.atom_id(label)?;

        if let Some(&(_, value)) = state.conds.iter().find(|(a, _)| *a == id) {
            return Ok(vec![(state, value)]);
        }

        let mut true_state = state.clone();
        true_state.conds.push((id, true));
        let mut false_state = state;
        false_state.conds.push((id, false));
        Ok(vec![(true_state, true), (false_state, false)])
    }

    /// Try to assign `label = value` on a path. Returns false when the path
    /// already pinned the atom to the opposite value (infeasible fork).
    fn assign_atom(
        &mut self,
        label: String,
        value: bool,
        state: &mut State,
    ) -> Result<bool, String> {
        let id = self.atom_id(label)?;
        if let Some(&(_, existing)) = state.conds.iter().find(|(a, _)| *a == id) {
            return Ok(existing == value);
        }
        state.conds.push((id, value));
        Ok(true)
    }

    /// Model a clean `switch` with else-if-chain semantics: entering case k
    /// means every earlier case value compared false and case k compared
    /// true. Sequential atoms keep mutually exclusive cases feasible-only.
    ///
    /// Returns `None` when the switch cannot be modeled faithfully
    /// (fall-through between groups, or a conditional `break`).
    fn model_switch(
        &mut self,
        node: tree_sitter::Node,
        state: &State,
    ) -> Result<Option<Vec<State>>, String> {
        let Some(cond) = node.child_by_field_name("condition") else {
            return Ok(None);
        };
        let subject = normalize(text(cond, self.src).trim_matches(['(', ')']));
        let Some(body) = node.child_by_field_name("body") else {
            return Ok(None);
        };

        // Phase 1: collect raw groups (case values — empty = default,
        // statements including any trailing break, arrow-style flag).
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
                                group.values.push(normalize(&text(e, self.src)));
                            }
                        } else {
                            group.stmts.push(part);
                        }
                    }
                    if group.values.is_empty() && !group.is_default {
                        return Ok(None); // unexpected shape
                    }
                    raw.push(group);
                }
                _ => {}
            }
        }
        if raw.is_empty() {
            return Ok(None);
        }

        // Phase 2: stacked labels (`case 1:` with no statements falling into
        // the next labeled group) — fold their values into that group.
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

        // Phase 3: faithfulness check per executable group + strip breaks.
        let mut groups: Vec<(Vec<String>, bool, Vec<tree_sitter::Node>)> = Vec::new();
        let merged_len = merged.len();
        for (i, mut group) in merged.into_iter().enumerate() {
            if !group.is_rule {
                let breaks = count_breaks(&group.stmts);
                let ends_with_break = group
                    .stmts
                    .last()
                    .is_some_and(|s| s.kind() == "break_statement");
                if breaks > 1 || (breaks == 1 && !ends_with_break) {
                    return Ok(None); // conditional break
                }
                if ends_with_break {
                    group.stmts.pop();
                } else {
                    let exits_always = group.stmts.last().is_some_and(|s| {
                        s.kind() == "return_statement" || s.kind() == "throw_statement"
                    });
                    // fall-through to the END of the switch is fine;
                    // fall-through into the next group is not modeled
                    if !exits_always && i + 1 != merged_len {
                        return Ok(None);
                    }
                }
            }
            groups.push((group.values, group.is_default, group.stmts));
        }

        let mut exits = Vec::new();
        let default_group: Option<usize> = groups.iter().position(|(_, is_default, _)| *is_default);

        // One fork per case value: earlier values false, this value true.
        let mut seen: Vec<String> = Vec::new();
        for (values, _, stmts) in groups.clone() {
            for value in values {
                let label = format!("{subject} == {value}");
                let mut branch = state.clone();
                let mut feasible = true;
                for earlier in &seen {
                    if !self.assign_atom(format!("{subject} == {earlier}"), false, &mut branch)? {
                        feasible = false;
                        break;
                    }
                }
                if feasible && self.assign_atom(label, true, &mut branch)? {
                    exits.extend(self.walk(&stmts, 0, branch)?);
                }
                seen.push(value);
            }
        }

        // The all-values-false fork: default group if present, else skip over.
        let mut fallback = state.clone();
        let mut feasible = true;
        for value in &seen {
            if !self.assign_atom(format!("{subject} == {value}"), false, &mut fallback)? {
                feasible = false;
                break;
            }
        }
        if feasible {
            match default_group {
                Some(gi) => {
                    let (_, _, stmts) = groups[gi].clone();
                    exits.extend(self.walk(&stmts, 0, fallback)?);
                }
                None => exits.push(fallback),
            }
        }

        Ok(Some(exits))
    }
}

/// Logging receivers conventional in (legacy) Java. Branches that only log
/// are treated as having no observable effect — for spec extraction that is
/// the desired reading, and it keeps outcome classes compressible.
const LOGGING_PREFIXES: &[&str] = &[
    "logger.",
    "log.",
    "LOG.",
    "Logger.",
    "System.out.",
    "System.err.",
];

fn is_logging_call(call: &str) -> bool {
    LOGGING_PREFIXES.iter().any(|p| call.starts_with(p))
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

fn parameter_names(method: tree_sitter::Node, src: &[u8]) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    if let Some(params) = method.child_by_field_name("parameters") {
        let mut cursor = params.walk();
        for param in params.named_children(&mut cursor) {
            if let Some(name) = param.child_by_field_name("name") {
                names.insert(text(name, src));
            }
        }
    }
    names
}

fn named_children(node: tree_sitter::Node) -> Vec<tree_sitter::Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|n| n.kind() != "line_comment" && n.kind() != "block_comment")
        .collect()
}

/// Statement list of a branch body: a block's children, or the single statement.
fn statements_of(node: tree_sitter::Node) -> Vec<tree_sitter::Node> {
    if node.kind() == "block" {
        named_children(node)
    } else {
        vec![node]
    }
}

fn contains_exit(node: tree_sitter::Node) -> bool {
    if node.kind() == "return_statement" || node.kind() == "throw_statement" {
        return true;
    }
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();
    children.into_iter().any(contains_exit)
}

fn text(node: tree_sitter::Node, src: &[u8]) -> String {
    node.utf8_text(src).unwrap_or_default().to_string()
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use specgraphen_logic::compress;

    fn extract(java: &str) -> DecisionExtraction {
        DecisionExtractor::new().unwrap().extract(java).unwrap()
    }

    fn single(java: &str) -> MethodDecision {
        let mut e = extract(java);
        assert_eq!(e.methods.len(), 1, "skipped: {:?}", e.skipped);
        e.methods.remove(0)
    }

    #[test]
    fn simple_if_else() {
        let m = single(
            r#"
            package com.example;
            class A {
                String m(boolean a) {
                    if (a) { return "x"; } else { return "y"; }
                }
            }
            "#,
        );
        assert_eq!(m.fqn(), "com.example.A.m");
        assert_eq!(m.table.variables(), ["a"]);
        assert_eq!(m.table.rows().len(), 2);
        assert!(!m.incomplete);
    }

    #[test]
    fn redundant_nesting_compresses_to_dead_variable() {
        // `b` is checked but both branches do the same thing
        let m = single(
            r#"
            class A {
                String m(boolean a, boolean b) {
                    if (a) {
                        if (b) { return "x"; } else { return "x"; }
                    }
                    return "y";
                }
            }
            "#,
        );
        let c = compress(&m.table).unwrap();
        assert_eq!(c.dead_variables, vec!["b"]);
        assert_eq!(c.rules.len(), 2);
    }

    #[test]
    fn short_circuit_and_decomposes_paths() {
        let m = single(
            r#"
            class A {
                String m(boolean a, boolean b) {
                    if (a && b) { return "x"; }
                    return "y";
                }
            }
            "#,
        );
        assert_eq!(m.table.variables(), ["a", "b"]);
        // paths: a=T,b=T → x; a=F → y; a=T,b=F → y
        assert_eq!(m.table.rows().len(), 3);
        let y_rows: Vec<_> = m
            .table
            .rows()
            .iter()
            .filter(|r| r.outcome == "return \"y\"")
            .collect();
        assert_eq!(y_rows.len(), 2);
        // the a=F path must not constrain b (short-circuit)
        assert!(y_rows
            .iter()
            .any(|r| r.inputs == vec![Tri::False, Tri::Any]));
    }

    #[test]
    fn negation_flips_branch_value() {
        let m = single(
            r#"
            class A {
                String m(boolean a) {
                    if (!a) { return "x"; }
                    return "y";
                }
            }
            "#,
        );
        assert_eq!(m.table.variables(), ["a"]);
        let x_row = m
            .table
            .rows()
            .iter()
            .find(|r| r.outcome == "return \"x\"")
            .unwrap();
        assert_eq!(x_row.inputs, vec![Tri::False]);
    }

    #[test]
    fn repeated_condition_follows_assigned_value() {
        // The second `if (a)` is unreachable with a different value:
        // no path may yield "y" with a=F, and a=T already returned.
        let m = single(
            r#"
            class A {
                String m(boolean a) {
                    if (a) { return "x"; }
                    if (a) { return "y"; }
                    return "z";
                }
            }
            "#,
        );
        assert!(
            !m.table.rows().iter().any(|r| r.outcome == "return \"y\""),
            "infeasible path was enumerated: {:?}",
            m.table.rows()
        );
        assert_eq!(m.table.rows().len(), 2); // x (a=T), z (a=F)
    }

    #[test]
    fn else_if_chain() {
        let m = single(
            r#"
            class A {
                int m(boolean a, boolean b) {
                    if (a) { return 1; } else if (b) { return 2; }
                    return 3;
                }
            }
            "#,
        );
        assert_eq!(m.table.rows().len(), 3);
        let c = compress(&m.table).unwrap();
        assert!(c.dead_variables.is_empty());
    }

    #[test]
    fn throw_is_an_outcome() {
        let m = single(
            r#"
            class A {
                void m(boolean valid) {
                    if (!valid) { throw new IllegalArgumentException("bad"); }
                }
            }
            "#,
        );
        let outcomes: Vec<_> = m.table.rows().iter().map(|r| r.outcome.as_str()).collect();
        assert!(outcomes
            .iter()
            .any(|o| o.starts_with("throw new IllegalArgumentException")));
        assert!(outcomes.contains(&"(fall-through)"));
    }

    #[test]
    fn loop_with_return_marks_incomplete() {
        let m = single(
            r#"
            class A {
                String m(boolean a, java.util.List<String> xs) {
                    if (a) { return "x"; }
                    for (String x : xs) { if (x.isEmpty()) return "found"; }
                    return "y";
                }
            }
            "#,
        );
        assert!(m.incomplete);
    }

    #[test]
    fn loop_without_exit_is_harmless() {
        let m = single(
            r#"
            class A {
                String m(boolean a, java.util.List<String> xs) {
                    if (a) { return "x"; }
                    for (String x : xs) { System.out.println(x); }
                    return "y";
                }
            }
            "#,
        );
        assert!(!m.incomplete);
        assert_eq!(m.table.rows().len(), 2);
    }

    #[test]
    fn non_branching_method_is_not_reported() {
        let e = extract(
            r#"
            class A {
                int m() { return 1; }
            }
            "#,
        );
        assert!(e.methods.is_empty());
        assert!(e.skipped.is_empty());
    }

    #[test]
    fn too_many_atoms_is_skipped_with_reason() {
        let conditions: String = (0..17)
            .map(|i| format!("if (c{i}) {{ return {i}; }}\n"))
            .collect();
        let java = format!("class A {{ int m() {{ {conditions} return -1; }} }}");
        let e = extract(&java);
        assert!(e.methods.is_empty());
        assert_eq!(e.skipped.len(), 1);
        assert!(e.skipped[0].0.ends_with("A.m"));
        assert!(e.skipped[0].1.contains("distinct conditions"));
    }

    #[test]
    fn assignment_to_returned_variable_distinguishes_outcomes() {
        // The BF-batch validation pattern: branches only assign, single return
        let m = single(
            r#"
            class A {
                int m(boolean c1, boolean c2) {
                    int a = 0;
                    if (c1) { a = -1; }
                    if (c2) { a = -1; }
                    return a;
                }
            }
            "#,
        );
        let outcomes: std::collections::HashSet<_> =
            m.table.rows().iter().map(|r| r.outcome.clone()).collect();
        assert!(outcomes.contains("return -1"), "{outcomes:?}");
        assert!(outcomes.contains("return 0"), "{outcomes:?}");
        let c = compress(&m.table).unwrap();
        assert!(
            c.dead_variables.is_empty(),
            "conditions wrongly dead: {:?}",
            c.dead_variables
        );
    }

    #[test]
    fn declaration_initializer_feeds_substitution() {
        let m = single(
            r#"
            class A {
                String m(boolean c) {
                    String r = "A";
                    if (c) { r = "B"; }
                    return r;
                }
            }
            "#,
        );
        let outcomes: Vec<_> = m.table.rows().iter().map(|r| r.outcome.as_str()).collect();
        assert!(outcomes.contains(&"return \"B\""), "{outcomes:?}");
        assert!(outcomes.contains(&"return \"A\""), "{outcomes:?}");
    }

    #[test]
    fn field_write_is_an_observable_outcome() {
        // Setter pattern: previously reported the condition as falsely dead
        let m = single(
            r#"
            class A {
                void setKbn(String x) {
                    if (x.equals("1")) { this.flag = true; }
                }
            }
            "#,
        );
        let c = compress(&m.table).unwrap();
        assert!(
            c.dead_variables.is_empty(),
            "field-writing condition wrongly dead: {:?}",
            c.dead_variables
        );
        assert!(
            m.table
                .rows()
                .iter()
                .any(|r| r.outcome.contains("this.flag = true")),
            "{:?}",
            m.table.rows()
        );
    }

    #[test]
    fn local_only_write_stays_invisible() {
        // A branch that only touches a local has no observable effect:
        // the condition is genuinely dead and must stay so.
        let m = single(
            r#"
            class A {
                void m(boolean c) {
                    int tmp = 0;
                    if (c) { tmp = 1; }
                }
            }
            "#,
        );
        let c = compress(&m.table).unwrap();
        assert_eq!(c.dead_variables, vec!["c"]);
    }

    #[test]
    fn parameter_write_stays_invisible() {
        let m = single(
            r#"
            class A {
                void m(boolean c, int a) {
                    if (c) { a = 1; }
                }
            }
            "#,
        );
        let c = compress(&m.table).unwrap();
        assert_eq!(c.dead_variables, vec!["c"]);
    }

    #[test]
    fn call_only_branch_is_observable() {
        // The legacy error-exit pattern: the branch only logs and calls an
        // exit helper. Previously reported the condition as falsely dead.
        let m = single(
            r#"
            class A {
                void check(String dir) {
                    if (dir.equals("")) {
                        logger.error("missing");
                        abortOnError();
                    }
                }
            }
            "#,
        );
        let c = compress(&m.table).unwrap();
        assert!(c.dead_variables.is_empty(), "{:?}", c.dead_variables);
        assert!(
            m.table
                .rows()
                .iter()
                .any(|r| r.outcome.contains("abortOnError()")),
            "{:?}",
            m.table.rows()
        );
    }

    #[test]
    fn clean_switch_models_as_sequential_cases() {
        let m = single(
            r#"
            class A {
                String m(String code) {
                    switch (code) {
                        case "A": return "alpha";
                        case "B": return "beta";
                        default: return "other";
                    }
                }
            }
            "#,
        );
        assert!(!m.incomplete);
        assert_eq!(m.table.variables(), ["code == \"A\"", "code == \"B\""]);
        assert_eq!(m.table.rows().len(), 3);
        let beta = m
            .table
            .rows()
            .iter()
            .find(|r| r.outcome == "return \"beta\"")
            .unwrap();
        // sequential semantics: case "B" implies case "A" compared false
        assert_eq!(beta.inputs, vec![Tri::False, Tri::True]);
        let c = compress(&m.table).unwrap();
        assert!(c.dead_variables.is_empty());
    }

    #[test]
    fn switch_with_breaks_and_assignments() {
        let m = single(
            r#"
            class A {
                int m(int code) {
                    int r = 0;
                    switch (code) {
                        case 1:
                            r = 10;
                            break;
                        case 2:
                            r = 20;
                            break;
                        default:
                            r = -1;
                            break;
                    }
                    return r;
                }
            }
            "#,
        );
        assert!(!m.incomplete);
        let outcomes: std::collections::HashSet<_> =
            m.table.rows().iter().map(|r| r.outcome.clone()).collect();
        assert!(outcomes.contains("return 10"), "{outcomes:?}");
        assert!(outcomes.contains("return 20"), "{outcomes:?}");
        assert!(outcomes.contains("return -1"), "{outcomes:?}");
    }

    #[test]
    fn stacked_case_labels_share_a_body() {
        let m = single(
            r#"
            class A {
                String m(int x) {
                    switch (x) {
                        case 1:
                        case 2:
                            return "low";
                        default:
                            return "high";
                    }
                }
            }
            "#,
        );
        assert!(!m.incomplete);
        let low: Vec<_> = m
            .table
            .rows()
            .iter()
            .filter(|r| r.outcome == "return \"low\"")
            .collect();
        assert_eq!(low.len(), 2, "{:?}", m.table.rows());
    }

    #[test]
    fn switch_without_default_can_skip_through() {
        let m = single(
            r#"
            class A {
                String m(int x) {
                    switch (x) {
                        case 1: return "one";
                    }
                    return "other";
                }
            }
            "#,
        );
        assert!(!m.incomplete);
        let outcomes: Vec<_> = m.table.rows().iter().map(|r| r.outcome.as_str()).collect();
        assert!(outcomes.contains(&"return \"one\""), "{outcomes:?}");
        assert!(outcomes.contains(&"return \"other\""), "{outcomes:?}");
    }

    #[test]
    fn fall_through_switch_is_not_modeled() {
        // case 1 falls into case 2 (no break) — faithful modeling is not
        // possible, so the old behavior applies: incomplete, no fabrication.
        let m = single(
            r#"
            class A {
                String m(int x, boolean c) {
                    if (c) { return "early"; }
                    switch (x) {
                        case 1:
                            doOne();
                        case 2:
                            doTwo();
                            return "fell";
                        default:
                            return "other";
                    }
                }
            }
            "#,
        );
        assert!(m.incomplete);
        // switch atoms must not appear
        assert_eq!(m.table.variables(), ["c"]);
    }

    #[test]
    fn conditional_break_switch_is_not_modeled() {
        let m = single(
            r#"
            class A {
                int m(int x, boolean c) {
                    int r = 0;
                    if (c) { r = 1; }
                    switch (x) {
                        case 1:
                            if (r == 1) break;
                            return 99;
                        default:
                            break;
                    }
                    return r;
                }
            }
            "#,
        );
        assert!(m.incomplete);
    }

    #[test]
    fn arrow_switch_is_clean_by_construction() {
        let m = single(
            r#"
            class A {
                String m(int x) {
                    switch (x) {
                        case 1 -> { return "one"; }
                        default -> { return "other"; }
                    }
                }
            }
            "#,
        );
        assert!(!m.incomplete);
        assert_eq!(m.table.rows().len(), 2);
    }

    #[test]
    fn system_exit_terminates_the_path() {
        let m = single(
            r#"
            class A {
                String m(boolean c) {
                    if (c) { System.exit(1); }
                    return "ok";
                }
            }
            "#,
        );
        let outcomes: Vec<_> = m.table.rows().iter().map(|r| r.outcome.as_str()).collect();
        assert!(outcomes.contains(&"System.exit(1)"), "{outcomes:?}");
        // the exiting path must NOT also show the unreachable return
        assert!(
            !outcomes
                .iter()
                .any(|o| o.contains("exit") && o.contains("ok")),
            "{outcomes:?}"
        );
    }

    #[test]
    fn configured_terminal_call_terminates_the_path() {
        let java = r#"
            class A {
                String m(boolean c) {
                    if (c) { abortOnError(); }
                    return "ok";
                }
            }
        "#;
        let mut e = DecisionExtractor::new()
            .unwrap()
            .with_terminal_calls(vec!["abortOnError".to_string()]);
        let mut x = e.extract(java).unwrap();
        let m = x.methods.remove(0);
        let outcomes: Vec<_> = m.table.rows().iter().map(|r| r.outcome.as_str()).collect();
        assert!(outcomes.contains(&"abortOnError()"), "{outcomes:?}");
        assert!(outcomes.contains(&"return \"ok\""), "{outcomes:?}");
    }

    #[test]
    fn unconfigured_helper_call_does_not_terminate() {
        // Without configuration the same call is just an effect; the path
        // continues to the return (the pre-2b behavior, still the default).
        let m = single(
            r#"
            class A {
                String m(boolean c) {
                    if (c) { abortOnError(); }
                    return "ok";
                }
            }
            "#,
        );
        assert!(
            m.table
                .rows()
                .iter()
                .any(|r| r.outcome.contains("abortOnError()") && r.outcome.contains("ok")),
            "{:?}",
            m.table.rows()
        );
    }

    #[test]
    fn logging_only_branch_reads_as_no_effect() {
        // A branch that only logs is not business behavior: for spec
        // extraction the condition is correctly reported dead.
        let m = single(
            r#"
            class A {
                void m(boolean c) {
                    if (c) { logger.warn("odd"); }
                }
            }
            "#,
        );
        let c = compress(&m.table).unwrap();
        assert_eq!(c.dead_variables, vec!["c"]);
    }

    #[test]
    fn effects_common_to_all_paths_are_not_shown() {
        // Both paths write this.audit and call init(); only the branch-
        // dependent write should appear in outcome labels.
        let m = single(
            r#"
            class A {
                void m(boolean c) {
                    init();
                    this.audit = "yes";
                    if (c) { this.flag = true; }
                }
            }
            "#,
        );
        for row in m.table.rows() {
            assert!(!row.outcome.contains("audit"), "{}", row.outcome);
            assert!(!row.outcome.contains("init()"), "{}", row.outcome);
        }
        assert!(m
            .table
            .rows()
            .iter()
            .any(|r| r.outcome.contains("this.flag = true")));
    }

    #[test]
    fn extracted_table_round_trips_through_compress() {
        // End-to-end: legacy-looking method where one flag is patch noise
        let m = single(
            r#"
            package com.example;
            class Pricing {
                int discount(boolean member, boolean legacyFlag, boolean campaign) {
                    if (member) {
                        if (legacyFlag) {
                            if (campaign) { return 20; } else { return 10; }
                        } else {
                            if (campaign) { return 20; } else { return 10; }
                        }
                    }
                    if (campaign) { return 5; }
                    return 0;
                }
            }
            "#,
        );
        let c = compress(&m.table).unwrap();
        assert_eq!(c.dead_variables, vec!["legacyFlag"]);
        let md = c.to_markdown();
        assert!(md.contains("member"), "{md}");
        assert!(!md.contains("| legacyFlag |"), "{md}");
    }
}
