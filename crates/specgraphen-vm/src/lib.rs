//! # specgraphen-vm
//!
//! Runtime-less symbolic execution over Java method ASTs.
//!
//! This crate is the shared execution core behind two consumers:
//!
//! - **batch decision-table extraction** (`specgraphen-lift`), which drives the
//!   semantics to enumerate every path through a method, and
//! - **interactive static debugging** (a GUI front-end), which drives the same
//!   semantics one step at a time, pausing at branches for a human or agent to
//!   choose a world line.
//!
//! [`sym`] holds the language semantics (condition evaluation, symbolic writes,
//! terminal-call detection) over a [`sym::SymState`]; [`stepper`] is the
//! resumable, explicit-stack driver built on top.
//!
//! The crate depends only on `tree-sitter` (the `Node` API), not on any
//! grammar or on the rest of specgraphen, so it stays a small reusable core.

pub mod enumerate;
pub mod stepper;
pub mod sym;

pub use enumerate::{enumerate, Outcome, SpecTable, WorldLine};
pub use stepper::{Frame, Stepper, Stop, StopKind};
pub use sym::{
    bind_local, invalidate, is_logging_call, is_terminal_call, named_children, normalize,
    parse_switch, record_write, text, AtomTable, Evaluator, SwitchGroup, SwitchModel, SymError,
    SymState,
};
