//! # specgraphen-logic
//!
//! Pure boolean decision-table compression for legacy spec extraction.
//!
//! Given observed branch paths — each a (possibly partial) assignment of
//! boolean conditions plus an outcome — this crate produces a minimal
//! decision table: the smallest set of rules that reproduces every observed
//! behavior, treating unobserved input combinations as don't-cares
//! (Quine-McCluskey minimization per outcome class). Conditions that no
//! surviving rule consults are reported as *dead variables*: they provably
//! never influence any observed outcome.
//!
//! This crate is deliberately free of I/O, AST, and graph concepts: inputs
//! are plain tables of [`Tri`] values, outputs are plain rule lists.
//!
//! ## Semantics
//!
//! - Rules are exact on **observed** behavior: every observed input
//!   combination is matched only by rules carrying its outcome.
//! - On **unobserved** combinations, rules of different outcomes may overlap;
//!   that region was free for the minimizer (don't care) and the table makes
//!   no claim about it.
//! - Two observed paths assigning the same complete input combination to
//!   different outcomes are a contradiction in the source material and are
//!   reported as [`Error::Conflict`] rather than silently resolved.
//!
//! ## Limits
//!
//! Minimization is exponential in the variable count; tables are capped at
//! [`MAX_VARIABLES`] variables and rejected beyond that. Partition large
//! condition sets upstream before calling [`compress`].

mod qm;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use qm::{prime_implicants, select_cover, Cube};

/// Upper bound on variables per table (minterm space is `2^n`).
pub const MAX_VARIABLES: usize = 16;

/// Value of one condition along one observed path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tri {
    True,
    False,
    /// Not evaluated / irrelevant on this path (don't care).
    Any,
}

/// One observed path: condition values plus the outcome it produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub inputs: Vec<Tri>,
    pub outcome: String,
}

/// An uncompressed decision table: named conditions and observed rows.
#[derive(Debug, Clone, Default)]
pub struct DecisionTable {
    variables: Vec<String>,
    rows: Vec<Row>,
}

/// One rule of a compressed table, aligned with the table's variable order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub when: Vec<Tri>,
    pub outcome: String,
}

/// The compressed decision table produced by [`compress`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressedTable {
    /// Variable names, same order as the input table.
    pub variables: Vec<String>,
    /// Minimized rules, sorted by outcome label then by rule shape.
    pub rules: Vec<Rule>,
    /// Variables no rule consults: provably irrelevant to every observed
    /// outcome under don't-care freedom for unobserved inputs.
    pub dead_variables: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    EmptyTable,
    TooManyVariables {
        count: usize,
        max: usize,
    },
    ArityMismatch {
        row: usize,
        expected: usize,
        got: usize,
    },
    /// The same complete input combination was observed with two different
    /// outcomes — a contradiction in the analyzed logic.
    Conflict {
        assignment: Vec<(String, bool)>,
        outcomes: (String, String),
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTable => write!(f, "decision table has no rows"),
            Self::TooManyVariables { count, max } => {
                write!(f, "{count} variables exceed the supported maximum of {max}")
            }
            Self::ArityMismatch { row, expected, got } => {
                write!(f, "row {row} has {got} inputs, expected {expected}")
            }
            Self::Conflict {
                assignment,
                outcomes,
            } => {
                let assign: Vec<String> = assignment
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect();
                write!(
                    f,
                    "conflicting outcomes '{}' and '{}' for assignment {{{}}}",
                    outcomes.0,
                    outcomes.1,
                    assign.join(", ")
                )
            }
        }
    }
}

impl std::error::Error for Error {}

impl DecisionTable {
    pub fn new(variables: Vec<String>) -> Self {
        Self {
            variables,
            rows: Vec::new(),
        }
    }

    /// Add an observed path. `inputs` must match the variable count.
    pub fn add_row(&mut self, inputs: Vec<Tri>, outcome: impl Into<String>) -> Result<(), Error> {
        if inputs.len() != self.variables.len() {
            return Err(Error::ArityMismatch {
                row: self.rows.len(),
                expected: self.variables.len(),
                got: inputs.len(),
            });
        }
        self.rows.push(Row {
            inputs,
            outcome: outcome.into(),
        });
        Ok(())
    }

    pub fn variables(&self) -> &[String] {
        &self.variables
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }
}

/// Compress a decision table to its minimal rule set.
///
/// Per outcome class, the ON-set is that class's observed minterms and the
/// don't-care set is everything never observed; Quine-McCluskey yields the
/// prime implicants and an essential-first cover selects the rules.
pub fn compress(table: &DecisionTable) -> Result<CompressedTable, Error> {
    let n = table.variables.len();
    if table.rows.is_empty() {
        return Err(Error::EmptyTable);
    }
    if n > MAX_VARIABLES {
        return Err(Error::TooManyVariables {
            count: n,
            max: MAX_VARIABLES,
        });
    }

    // Expand rows into minterm → outcome, detecting contradictions.
    let mut outcome_labels: Vec<String> = Vec::new();
    let mut outcome_index: HashMap<String, usize> = HashMap::new();
    let mut observed: BTreeMap<u32, usize> = BTreeMap::new();

    for row in &table.rows {
        let outcome = *outcome_index.entry(row.outcome.clone()).or_insert_with(|| {
            outcome_labels.push(row.outcome.clone());
            outcome_labels.len() - 1
        });

        let mut base = 0u32;
        let mut free: Vec<usize> = Vec::new();
        for (i, tri) in row.inputs.iter().enumerate() {
            match tri {
                Tri::True => base |= 1 << i,
                Tri::False => {}
                Tri::Any => free.push(i),
            }
        }

        for combo in 0u32..(1 << free.len()) {
            let mut minterm = base;
            for (j, &pos) in free.iter().enumerate() {
                if combo >> j & 1 == 1 {
                    minterm |= 1 << pos;
                }
            }
            if let Some(&existing) = observed.get(&minterm) {
                if existing != outcome {
                    return Err(Error::Conflict {
                        assignment: table
                            .variables
                            .iter()
                            .enumerate()
                            .map(|(i, name)| (name.clone(), minterm >> i & 1 == 1))
                            .collect(),
                        outcomes: (
                            outcome_labels[existing].clone(),
                            outcome_labels[outcome].clone(),
                        ),
                    });
                }
            } else {
                observed.insert(minterm, outcome);
            }
        }
    }

    // Minimize each outcome class against the others, with unobserved
    // combinations as shared don't-cares.
    let space: u32 = 1 << n;
    let mut sorted_outcomes: Vec<(String, usize)> = outcome_labels
        .iter()
        .cloned()
        .enumerate()
        .map(|(i, label)| (label, i))
        .collect();
    sorted_outcomes.sort();

    let mut rules: Vec<Rule> = Vec::new();
    for (label, idx) in &sorted_outcomes {
        let on: Vec<u32> = observed
            .iter()
            .filter(|(_, &o)| o == *idx)
            .map(|(&m, _)| m)
            .collect();
        let off: HashSet<u32> = observed
            .iter()
            .filter(|(_, &o)| o != *idx)
            .map(|(&m, _)| m)
            .collect();
        let care: HashSet<u32> = (0..space).filter(|m| !off.contains(m)).collect();

        let primes = prime_implicants(&care, n);
        for cube in select_cover(&primes, &on) {
            rules.push(Rule {
                when: cube_to_tris(cube, n),
                outcome: label.clone(),
            });
        }
    }

    let dead_variables: Vec<String> = (0..n)
        .filter(|&i| rules.iter().all(|r| r.when[i] == Tri::Any))
        .map(|i| table.variables[i].clone())
        .collect();

    Ok(CompressedTable {
        variables: table.variables.clone(),
        rules,
        dead_variables,
    })
}

fn cube_to_tris(cube: Cube, n: usize) -> Vec<Tri> {
    (0..n)
        .map(|i| {
            if cube.mask >> i & 1 == 0 {
                Tri::Any
            } else if cube.bits >> i & 1 == 1 {
                Tri::True
            } else {
                Tri::False
            }
        })
        .collect()
}

impl CompressedTable {
    /// Live (non-dead) variables in table order.
    pub fn live_variables(&self) -> Vec<&str> {
        self.variables
            .iter()
            .filter(|v| !self.dead_variables.contains(v))
            .map(String::as_str)
            .collect()
    }

    /// Render as a Markdown table. Dead variables are omitted from the
    /// columns and listed below the table.
    pub fn to_markdown(&self) -> String {
        let live: Vec<usize> = (0..self.variables.len())
            .filter(|&i| !self.dead_variables.contains(&self.variables[i]))
            .collect();

        let mut out = String::new();
        out.push('|');
        for &i in &live {
            out.push_str(&format!(" {} |", self.variables[i]));
        }
        out.push_str(" outcome |\n|");
        for _ in &live {
            out.push_str("---|");
        }
        out.push_str("---|\n");

        for rule in &self.rules {
            out.push('|');
            for &i in &live {
                let cell = match rule.when[i] {
                    Tri::True => "T",
                    Tri::False => "F",
                    Tri::Any => "-",
                };
                out.push_str(&format!(" {cell} |"));
            }
            out.push_str(&format!(" {} |\n", rule.outcome));
        }

        if !self.dead_variables.is_empty() {
            out.push_str(&format!(
                "\nDead variables (no effect on any observed outcome): {}\n",
                self.dead_variables.join(", ")
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Tri::{Any, False as F, True as T};

    fn table(vars: &[&str], rows: &[(&[Tri], &str)]) -> DecisionTable {
        let mut t = DecisionTable::new(vars.iter().map(|s| s.to_string()).collect());
        for (inputs, outcome) in rows {
            t.add_row(inputs.to_vec(), *outcome).unwrap();
        }
        t
    }

    /// Every observed minterm must be matched by at least one rule of its own
    /// outcome and by no rule of any other outcome.
    fn assert_faithful(t: &DecisionTable, c: &CompressedTable) {
        let n = t.variables().len();
        for row in t.rows() {
            let free: Vec<usize> = (0..n).filter(|&i| row.inputs[i] == Any).collect();
            let base: u32 = (0..n)
                .filter(|&i| row.inputs[i] == T)
                .fold(0, |acc, i| acc | 1 << i);
            for combo in 0u32..(1 << free.len()) {
                let mut m = base;
                for (j, &pos) in free.iter().enumerate() {
                    if combo >> j & 1 == 1 {
                        m |= 1 << pos;
                    }
                }
                let matches = |rule: &Rule| {
                    rule.when.iter().enumerate().all(|(i, tri)| match tri {
                        Tri::Any => true,
                        Tri::True => m >> i & 1 == 1,
                        Tri::False => m >> i & 1 == 0,
                    })
                };
                assert!(
                    c.rules
                        .iter()
                        .any(|r| r.outcome == row.outcome && matches(r)),
                    "minterm {m:b} of outcome {} unmatched",
                    row.outcome
                );
                assert!(
                    !c.rules
                        .iter()
                        .any(|r| r.outcome != row.outcome && matches(r)),
                    "minterm {m:b} matched by foreign rule"
                );
            }
        }
    }

    #[test]
    fn dead_variable_is_eliminated() {
        // outcome depends only on `a`; `b` is patch noise
        let t = table(
            &["a", "b"],
            &[
                (&[T, T], "yes"),
                (&[T, F], "yes"),
                (&[F, T], "no"),
                (&[F, F], "no"),
            ],
        );
        let c = compress(&t).unwrap();
        assert_faithful(&t, &c);
        assert_eq!(c.dead_variables, vec!["b"]);
        assert_eq!(c.rules.len(), 2);
        assert!(c.rules.iter().all(|r| r.when[1] == Any));
    }

    #[test]
    fn independent_variables_do_not_compress() {
        // XNOR: both variables genuinely matter
        let t = table(
            &["a", "b"],
            &[
                (&[T, T], "eq"),
                (&[F, F], "eq"),
                (&[T, F], "ne"),
                (&[F, T], "ne"),
            ],
        );
        let c = compress(&t).unwrap();
        assert_faithful(&t, &c);
        assert!(c.dead_variables.is_empty());
        assert_eq!(c.rules.len(), 4);
    }

    #[test]
    fn unobserved_combinations_are_dont_cares() {
        // (T,F) never observed; using it as don't-care kills `b` entirely
        let t = table(
            &["a", "b"],
            &[(&[T, T], "yes"), (&[F, T], "no"), (&[F, F], "no")],
        );
        let c = compress(&t).unwrap();
        assert_faithful(&t, &c);
        assert_eq!(c.dead_variables, vec!["b"]);
        assert_eq!(c.rules.len(), 2);
    }

    #[test]
    fn partial_rows_expand_to_dont_cares() {
        let t = table(&["a", "b"], &[(&[T, Any], "yes"), (&[F, Any], "no")]);
        let c = compress(&t).unwrap();
        assert_faithful(&t, &c);
        assert_eq!(c.dead_variables, vec!["b"]);
        assert_eq!(c.rules.len(), 2);
    }

    #[test]
    fn three_outcomes() {
        let t = table(
            &["a", "b"],
            &[(&[T, T], "A"), (&[T, F], "B"), (&[F, Any], "C")],
        );
        let c = compress(&t).unwrap();
        assert_faithful(&t, &c);
        assert!(c.dead_variables.is_empty());
        // A and B each need both variables; C needs only `a`
        let c_rules: Vec<_> = c.rules.iter().filter(|r| r.outcome == "C").collect();
        assert_eq!(c_rules.len(), 1);
        assert_eq!(c_rules[0].when, vec![F, Any]);
    }

    #[test]
    fn single_outcome_makes_everything_dead() {
        let t = table(&["a", "b"], &[(&[T, T], "ok"), (&[F, F], "ok")]);
        let c = compress(&t).unwrap();
        assert_faithful(&t, &c);
        assert_eq!(c.rules.len(), 1);
        assert_eq!(c.rules[0].when, vec![Any, Any]);
        assert_eq!(c.dead_variables, vec!["a", "b"]);
    }

    #[test]
    fn conflict_is_detected() {
        let t = table(&["a", "b"], &[(&[T, Any], "yes"), (&[T, T], "no")]);
        match compress(&t) {
            Err(Error::Conflict {
                assignment,
                outcomes,
            }) => {
                assert_eq!(
                    assignment,
                    vec![("a".to_string(), true), ("b".to_string(), true)]
                );
                assert_eq!(outcomes, ("yes".to_string(), "no".to_string()));
            }
            other => panic!("expected conflict, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_consistent_rows_are_fine() {
        let t = table(&["a"], &[(&[T], "yes"), (&[T], "yes"), (&[F], "no")]);
        let c = compress(&t).unwrap();
        assert_faithful(&t, &c);
        assert_eq!(c.rules.len(), 2);
    }

    #[test]
    fn empty_table_is_an_error() {
        let t = DecisionTable::new(vec!["a".to_string()]);
        assert_eq!(compress(&t), Err(Error::EmptyTable));
    }

    #[test]
    fn arity_mismatch_is_an_error() {
        let mut t = DecisionTable::new(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            t.add_row(vec![T], "yes"),
            Err(Error::ArityMismatch {
                row: 0,
                expected: 2,
                got: 1
            })
        );
    }

    #[test]
    fn too_many_variables_is_an_error() {
        let vars: Vec<String> = (0..17).map(|i| format!("v{i}")).collect();
        let mut t = DecisionTable::new(vars);
        t.add_row(vec![Any; 17], "x").unwrap();
        assert_eq!(
            compress(&t),
            Err(Error::TooManyVariables { count: 17, max: 16 })
        );
    }

    #[test]
    fn compression_is_deterministic() {
        let t = table(
            &["a", "b", "c"],
            &[
                (&[T, T, Any], "x"),
                (&[T, F, T], "y"),
                (&[F, Any, F], "x"),
                (&[F, T, T], "y"),
            ],
        );
        let c1 = compress(&t).unwrap();
        let c2 = compress(&t).unwrap();
        assert_eq!(c1, c2);
        assert_faithful(&t, &c1);
    }

    #[test]
    fn nested_redundant_conditions_collapse() {
        // Three nested ifs where only the innermost matters:
        //   if (a) { if (b) { if (c) X else Y } else { if (c) X else Y } } ...
        let t = table(
            &["a", "b", "c"],
            &[
                (&[T, T, T], "X"),
                (&[T, T, F], "Y"),
                (&[T, F, T], "X"),
                (&[T, F, F], "Y"),
                (&[F, T, T], "X"),
                (&[F, T, F], "Y"),
                (&[F, F, T], "X"),
                (&[F, F, F], "Y"),
            ],
        );
        let c = compress(&t).unwrap();
        assert_faithful(&t, &c);
        assert_eq!(c.dead_variables, vec!["a", "b"]);
        assert_eq!(c.rules.len(), 2);
    }

    #[test]
    fn markdown_rendering() {
        let t = table(
            &["isAdmin", "legacyFlag"],
            &[
                (&[T, T], "allow"),
                (&[T, F], "allow"),
                (&[F, T], "deny"),
                (&[F, F], "deny"),
            ],
        );
        let md = compress(&t).unwrap().to_markdown();
        assert!(md.contains("| isAdmin | outcome |"), "got:\n{md}");
        assert!(!md.contains("legacyFlag |"), "dead column leaked:\n{md}");
        assert!(md.contains("Dead variables"), "got:\n{md}");
        assert!(md.contains("legacyFlag"), "dead var not listed:\n{md}");
    }
}
