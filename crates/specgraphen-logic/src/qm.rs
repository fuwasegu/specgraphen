//! Quine-McCluskey logic minimization over bitmask minterms.

use std::collections::{BTreeSet, HashSet};

/// A product term over boolean variables, encoded as bitmasks.
/// Bit `i` of `mask` set means variable `i` is specified; `bits` holds the
/// specified values (bits outside `mask` are always zero).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Cube {
    pub bits: u32,
    pub mask: u32,
}

impl Cube {
    pub fn covers(&self, minterm: u32) -> bool {
        minterm & self.mask == self.bits
    }
}

/// All prime implicants of the function whose care-set (ON ∪ DC) is `care`.
///
/// Classic iterative combining: two cubes that specify the same variables and
/// differ in exactly one value merge into a cube with that variable freed.
/// Cubes that never merge are prime.
pub(crate) fn prime_implicants(care: &HashSet<u32>, n: usize) -> Vec<Cube> {
    let full_mask = if n == 0 { 0 } else { (1u32 << n) - 1 };
    let mut current: HashSet<Cube> = care
        .iter()
        .map(|&m| Cube {
            bits: m,
            mask: full_mask,
        })
        .collect();
    let mut primes: HashSet<Cube> = HashSet::new();

    while !current.is_empty() {
        let mut next: HashSet<Cube> = HashSet::new();
        let mut combined: HashSet<Cube> = HashSet::new();

        for &cube in &current {
            let mut was_combined = false;
            let mut remaining = cube.mask;
            while remaining != 0 {
                let bit = remaining & remaining.wrapping_neg();
                remaining &= remaining - 1;
                let partner = Cube {
                    bits: cube.bits ^ bit,
                    mask: cube.mask,
                };
                if current.contains(&partner) {
                    was_combined = true;
                    next.insert(Cube {
                        bits: cube.bits & !bit,
                        mask: cube.mask & !bit,
                    });
                }
            }
            if was_combined {
                combined.insert(cube);
            }
        }

        for cube in current {
            if !combined.contains(&cube) {
                primes.insert(cube);
            }
        }
        current = next;
    }

    let mut result: Vec<Cube> = primes.into_iter().collect();
    result.sort();
    result
}

/// Select a small set of prime implicants covering every ON minterm
/// (don't-cares need no cover). Essential primes are taken first; remaining
/// gaps are filled greedily preferring broad, general cubes. The result is
/// a correct cover, near-minimal in practice, and deterministic.
pub(crate) fn select_cover(primes: &[Cube], on: &[u32]) -> Vec<Cube> {
    let mut uncovered: BTreeSet<u32> = on.iter().copied().collect();
    let mut available: Vec<Cube> = primes.to_vec();
    let mut chosen: Vec<Cube> = Vec::new();

    while !uncovered.is_empty() {
        // A minterm covered by exactly one remaining prime makes it essential
        let mut pick: Option<Cube> = None;
        for &m in &uncovered {
            let mut covering = available.iter().filter(|c| c.covers(m));
            if let (Some(&only), None) = (covering.next(), covering.next()) {
                pick = Some(only);
                break;
            }
        }

        let pick = pick.unwrap_or_else(|| {
            *available
                .iter()
                .max_by_key(|c| {
                    let coverage = uncovered.iter().filter(|&&m| c.covers(m)).count();
                    (
                        coverage,
                        std::cmp::Reverse(c.mask.count_ones()), // prefer general cubes
                        std::cmp::Reverse(c.bits),              // deterministic tie-break
                        std::cmp::Reverse(c.mask),
                    )
                })
                .expect("every ON minterm is covered by some prime implicant")
        });

        uncovered.retain(|&m| !pick.covers(m));
        available.retain(|&c| c != pick);
        chosen.push(pick);
    }

    chosen.sort();
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn care(minterms: &[u32]) -> HashSet<u32> {
        minterms.iter().copied().collect()
    }

    #[test]
    fn single_minterm_is_its_own_prime() {
        let primes = prime_implicants(&care(&[0b10]), 2);
        assert_eq!(
            primes,
            vec![Cube {
                bits: 0b10,
                mask: 0b11
            }]
        );
    }

    #[test]
    fn adjacent_minterms_merge() {
        // {00, 01} → cube with bit0 freed
        let primes = prime_implicants(&care(&[0b00, 0b01]), 2);
        assert_eq!(
            primes,
            vec![Cube {
                bits: 0b00,
                mask: 0b10
            }]
        );
    }

    #[test]
    fn full_space_merges_to_tautology() {
        let primes = prime_implicants(&care(&[0, 1, 2, 3]), 2);
        assert_eq!(primes, vec![Cube { bits: 0, mask: 0 }]);
    }

    #[test]
    fn textbook_four_variable_example() {
        // f = Σm(4,8,10,11,12,15) + d(9,14) — classic worked example whose
        // minimal cover has 3 terms.
        let all: Vec<u32> = vec![4, 8, 10, 11, 12, 15, 9, 14];
        let on: Vec<u32> = vec![4, 8, 10, 11, 12, 15];
        let primes = prime_implicants(&care(&all), 4);
        let cover = select_cover(&primes, &on);
        assert_eq!(cover.len(), 3, "cover was {cover:?}");
        // Verify it is actually a cover of ON
        for &m in &on {
            assert!(cover.iter().any(|c| c.covers(m)), "minterm {m} uncovered");
        }
    }

    #[test]
    fn cover_is_deterministic() {
        let all: Vec<u32> = (0..16).filter(|m| m % 3 != 0).collect();
        let on: Vec<u32> = all.clone();
        let c = care(&all);
        let a = select_cover(&prime_implicants(&c, 4), &on);
        let b = select_cover(&prime_implicants(&c, 4), &on);
        assert_eq!(a, b);
    }
}
