//! Stateless, replayable symbolic-debug traces over a single Java method.
//!
//! A debug "session" is just the list of branch choices made so far: this
//! function re-parses the method and replays those choices deterministically
//! (execution is side-effect-free, so replay is exact and cheap). The caller
//! — an MCP tool or a GUI — holds only the `choices` array; undo is dropping
//! its last element, time-travel is truncating it.
//!
//! Each call advances from the start to the next decision point (or
//! termination), so the granularity an agent sees is "what happened until the
//! next branch", not one statement at a time.

use specgraphen_vm::stepper::{Stepper, StopKind};

/// Cap on statements executed per trace call (guards pathological methods).
const MAX_TRACE_STEPS: usize = 2000;

#[derive(Debug, Clone)]
pub struct TraceStep {
    pub line: u32,
    pub text: String,
}

#[derive(Debug, Clone)]
pub enum TraceStatus {
    /// Paused at a branch; choose an index (appended to `choices`) to continue.
    AwaitingChoice { branches: Vec<String> },
    /// The path ended.
    Terminated { outcome: String },
    /// Ran off the end of the method without an explicit return.
    FallThrough,
    /// Hit the per-call step cap before reaching a decision or end.
    StepCap,
}

#[derive(Debug, Clone)]
pub struct TraceResult {
    /// Statements executed since the start, in order.
    pub executed: Vec<TraceStep>,
    /// Current symbolic variable values (`target` → value text).
    pub variables: Vec<(String, String)>,
    /// Condition atoms pinned on this path ("Context Rules").
    pub context_rules: Vec<String>,
    /// Call-stack frame labels, outermost first.
    pub call_stack: Vec<String>,
    pub status: TraceStatus,
    /// True if this path stepped over an unmodeled construct that can itself
    /// exit the method (a `switch`/`try` with a return/throw inside) — the
    /// reported outcome may then not be the method's real behavior.
    pub incomplete: bool,
}

/// Replay `choices` through the method whose declaration starts at `start_line`
/// (1-based) in `source`, returning the resulting trace and current state.
pub fn trace(
    source: &str,
    start_line: u32,
    choices: &[usize],
    terminal_calls: &[String],
) -> anyhow::Result<TraceResult> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_java::LANGUAGE.into())?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("failed to parse source"))?;
    let src = source.as_bytes();

    let body = find_method_body(tree.root_node(), start_line)
        .ok_or_else(|| anyhow::anyhow!("no method declaration starts at line {start_line}"))?;

    let mut stepper = Stepper::start(
        body,
        src,
        terminal_calls.to_vec(),
        specgraphen_logic::MAX_VARIABLES,
    );

    let mut executed = Vec::new();
    let mut choice_iter = choices.iter().copied();
    let mut steps = 0usize;

    let status = loop {
        if stepper.is_paused_at_branch() {
            match choice_iter.next() {
                Some(choice) => {
                    stepper
                        .choose(choice)
                        .map_err(|e| anyhow::anyhow!("invalid choice {choice}: {e}"))?;
                    continue;
                }
                None => {
                    break TraceStatus::AwaitingChoice {
                        branches: stepper.peek_branches(),
                    };
                }
            }
        }

        if steps >= MAX_TRACE_STEPS {
            break TraceStatus::StepCap;
        }

        // Record the statement about to run, then advance.
        let loc = stepper.current().map(|n| TraceStep {
            line: n.start_position().row as u32 + 1,
            text: node_text(n, src),
        });
        let stop = stepper.step().map_err(|e| anyhow::anyhow!("{e}"))?;
        steps += 1;
        if let Some(step) = loc {
            executed.push(step);
        }
        match stop.kind {
            StopKind::Stepped | StopKind::Branch => continue,
            StopKind::Terminated => {
                break TraceStatus::Terminated {
                    outcome: stop.outcome.unwrap_or_default(),
                }
            }
            StopKind::FallThrough => break TraceStatus::FallThrough,
        }
    };

    Ok(TraceResult {
        executed,
        variables: stepper
            .state()
            .writes
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        context_rules: stepper.context_rules(),
        call_stack: stepper.call_stack().iter().map(|s| s.to_string()).collect(),
        status,
        incomplete: stepper.is_incomplete(),
    })
}

fn find_method_body(node: tree_sitter::Node, start_line: u32) -> Option<tree_sitter::Node> {
    if matches!(
        node.kind(),
        "method_declaration" | "constructor_declaration"
    ) && node.start_position().row as u32 + 1 == start_line
    {
        return node.child_by_field_name("body");
    }
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();
    for child in children {
        if let Some(body) = find_method_body(child, start_line) {
            return Some(body);
        }
    }
    None
}

fn node_text(node: tree_sitter::Node, src: &[u8]) -> String {
    let raw = node.utf8_text(src).unwrap_or_default();
    // First line only, whitespace-collapsed — trace lines stay scannable.
    let first = raw.lines().next().unwrap_or("");
    first.split_whitespace().collect::<Vec<_>>().join(" ")
}
