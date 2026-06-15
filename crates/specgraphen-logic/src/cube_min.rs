//! Cube-based heuristic minimization for tables too wide for exact QM.
//!
//! Exact Quine-McCluskey enumerates the 2^n minterm space, which is
//! infeasible past ~12 variables. Observed paths, however, already ARE
//! cubes ([`Tri`] vectors), so in the Espresso spirit we minimize without
//! ever enumerating: each ON cube is EXPANDed (literals freed one by one)
//! as far as it can go without intersecting any OFF cube — unobserved
//! space is implicitly don't-care, exactly as in the exact path — and an
//! essential-first greedy cover selects the rules.
//!
//! Cubes are packed into a pair of u64 masks (positive / negative
//! literals), so intersection and containment are O(1) and thousands of
//! paths stay fast.
//!
//! Guarantees: every rule is conflict-free with observed other-outcome
//! rows, every observed row is covered, and each rule is maximally
//! expanded (prime with respect to the OFF list). The cover is
//! near-minimal in practice but, unlike the exact path, not guaranteed
//! minimum.

use crate::Tri;

/// A product term over ≤64 variables: bit i of `pos` set = variable i must
/// be true; bit i of `neg` set = must be false; neither = don't care.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct BitCube {
    pub pos: u64,
    pub neg: u64,
}

impl BitCube {
    pub fn from_tris(tris: &[Tri]) -> Self {
        let mut cube = Self { pos: 0, neg: 0 };
        for (i, t) in tris.iter().enumerate() {
            match t {
                Tri::True => cube.pos |= 1 << i,
                Tri::False => cube.neg |= 1 << i,
                Tri::Any => {}
            }
        }
        cube
    }

    pub fn to_tris(self, n: usize) -> Vec<Tri> {
        (0..n)
            .map(|i| {
                if self.pos >> i & 1 == 1 {
                    Tri::True
                } else if self.neg >> i & 1 == 1 {
                    Tri::False
                } else {
                    Tri::Any
                }
            })
            .collect()
    }

    /// Do two cubes share at least one full assignment?
    pub fn intersects(self, other: Self) -> bool {
        self.pos & other.neg == 0 && self.neg & other.pos == 0
    }

    /// Is every assignment of `inner` matched by `self`?
    pub fn contains(self, inner: Self) -> bool {
        self.pos & !inner.pos == 0 && self.neg & !inner.neg == 0
    }

    /// A witness assignment inside the intersection of two cubes
    /// (unconstrained variables default to false).
    pub fn intersection_witness(self, other: Self, n: usize) -> Vec<bool> {
        let pos = self.pos | other.pos;
        (0..n).map(|i| pos >> i & 1 == 1).collect()
    }
}

/// Free as many literals of `cube` as possible without touching any OFF
/// cube. Deterministic: variables are tried in index order.
fn expand(mut cube: BitCube, off: &[BitCube], n: usize) -> BitCube {
    for i in 0..n {
        let bit = 1u64 << i;
        if cube.pos & bit == 0 && cube.neg & bit == 0 {
            continue;
        }
        let attempt = BitCube {
            pos: cube.pos & !bit,
            neg: cube.neg & !bit,
        };
        if !off.iter().any(|o| attempt.intersects(*o)) {
            cube = attempt;
        }
    }
    cube
}

/// Minimize one outcome class: expand every ON cube against the OFF list,
/// then select an essential-first greedy cover of the ON cubes.
pub(crate) fn minimize(on: &[BitCube], off: &[BitCube], n: usize) -> Vec<BitCube> {
    // Expand and deduplicate candidates
    let mut candidates: Vec<BitCube> = Vec::new();
    for &cube in on {
        let expanded = expand(cube, off, n);
        if !candidates.contains(&expanded) {
            candidates.push(expanded);
        }
    }

    // Drop candidates strictly contained in another candidate
    let snapshot = candidates.clone();
    candidates.retain(|c| {
        !snapshot
            .iter()
            .any(|other| other != c && other.contains(*c))
    });

    // Essential-first greedy cover of the ON cubes
    let mut uncovered: Vec<BitCube> = on.to_vec();
    uncovered.dedup();
    let mut available = candidates;
    let mut chosen: Vec<BitCube> = Vec::new();

    while !uncovered.is_empty() {
        // An ON cube covered by exactly one remaining candidate is essential
        let mut pick: Option<usize> = None;
        for &cube in &uncovered {
            let mut covering = available
                .iter()
                .enumerate()
                .filter(|(_, c)| c.contains(cube));
            if let (Some((idx, _)), None) = (covering.next(), covering.next()) {
                pick = Some(idx);
                break;
            }
        }

        let idx = pick.unwrap_or_else(|| {
            (0..available.len())
                .max_by_key(|&i| {
                    let coverage = uncovered
                        .iter()
                        .filter(|&&cube| available[i].contains(cube))
                        .count();
                    let generality = (available[i].pos | available[i].neg).count_zeros();
                    (coverage, generality, std::cmp::Reverse(i))
                })
                .expect("every ON cube is covered by its own expansion")
        });

        let picked = available.remove(idx);
        uncovered.retain(|&cube| !picked.contains(cube));
        chosen.push(picked);
    }

    chosen.sort_by_key(|c| ((c.pos | c.neg).count_ones(), c.pos, c.neg));
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;
    use Tri::{Any, False as F, True as T};

    fn cube(tris: &[Tri]) -> BitCube {
        BitCube::from_tris(tris)
    }

    #[test]
    fn round_trip() {
        let tris = vec![T, F, Any, T];
        assert_eq!(cube(&tris).to_tris(4), tris);
    }

    #[test]
    fn intersection_rules() {
        assert!(cube(&[T, Any]).intersects(cube(&[Any, F])));
        assert!(!cube(&[T, Any]).intersects(cube(&[F, Any])));
        assert!(cube(&[Any, Any]).intersects(cube(&[T, F])));
    }

    #[test]
    fn containment_rules() {
        assert!(cube(&[Any, Any]).contains(cube(&[T, F])));
        assert!(cube(&[T, Any]).contains(cube(&[T, F])));
        assert!(!cube(&[T, F]).contains(cube(&[T, Any]))); // inner is wider
    }

    #[test]
    fn expand_frees_unconstrained_literals() {
        // freeing var0 would hit OFF; freeing var1 is safe
        let expanded = expand(cube(&[T, T]), &[cube(&[F, Any])], 2);
        assert_eq!(expanded, cube(&[T, Any]));
    }

    #[test]
    fn minimize_collapses_symmetric_rows() {
        // f = a (b irrelevant): two ON rows differing only in b
        let rules = minimize(&[cube(&[T, T]), cube(&[T, F])], &[cube(&[F, Any])], 2);
        assert_eq!(rules, vec![cube(&[T, Any])]);
    }

    #[test]
    fn no_off_means_tautology() {
        let rules = minimize(&[cube(&[T, F, T])], &[], 3);
        assert_eq!(rules, vec![cube(&[Any, Any, Any])]);
    }

    #[test]
    fn scales_to_thousands_of_rows() {
        // 4000 ON cubes over 60 vars against 4000 OFF cubes — must finish
        // promptly (this is the monster-method scenario).
        let n = 60;
        let on: Vec<BitCube> = (0..4000u64)
            .map(|i| BitCube {
                pos: (1 << (i % n as u64)) | 1 << 59,
                neg: 1 << ((i + 7) % 59),
            })
            .collect();
        let off: Vec<BitCube> = (0..4000u64)
            .map(|i| BitCube {
                pos: 1 << ((i + 3) % 59),
                neg: 1 << 59,
            })
            .collect();
        let rules = minimize(&on, &off, n);
        assert!(!rules.is_empty());
        for rule in &rules {
            assert!(off.iter().all(|o| !rule.intersects(*o)));
        }
    }
}
