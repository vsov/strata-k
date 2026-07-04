//! Chain capture: derivations → a provenance circuit. [Phase 4]
//!
//! режим B records, for a queried tuple, *how* it is derived — the set of proofs,
//! each a conjunction of the probabilistic facts it rests on. Collecting those
//! proofs and compiling them to an OR-of-ANDs circuit ([`build_dnf`]) is the
//! provenance capture; weighted model counting on it (see [`crate::circuit`])
//! then gives the exact marginal — correct because the two-path convolution a
//! naive semiring does would double-count shared facts, while the circuit counts
//! each leaf once.

use crate::circuit::{Builder, Circuit};

/// Compile captured proofs into a provenance circuit: `OR` over proofs, each an
/// `AND` of the leaf facts supporting it. An empty proof is `true` (the tuple is
/// certain); no proofs is `false` (underivable).
pub fn build_dnf(proofs: &[Vec<usize>]) -> Circuit {
    let mut b = Builder::new();
    if proofs.is_empty() {
        let f = b.fals();
        return b.finish(f);
    }
    let disjuncts: Vec<usize> = proofs
        .iter()
        .map(|proof| {
            if proof.is_empty() {
                b.tru()
            } else {
                let leaves: Vec<usize> = proof.iter().map(|&l| b.leaf(l)).collect();
                if leaves.len() == 1 {
                    leaves[0]
                } else {
                    b.and(leaves)
                }
            }
        })
        .collect();
    let root = if disjuncts.len() == 1 {
        disjuncts[0]
    } else {
        b.or(disjuncts)
    };
    b.finish(root)
}

/// The MNIST-sum provenance for `sum == s`, over two independent digit
/// distributions. Leaves `0..10` are `digit1 = d` (prob `p1[d]`); leaves
/// `10..20` are `digit2 = d` (prob `p2[d]`). Each proof `digit1 = a ∧
/// digit2 = (s-a)` is a captured derivation; distinct `a` make the disjuncts
/// mutually exclusive, so WMC = Σ_a p1[a]·p2[s-a] exactly.
pub fn sum_circuit(s: usize, digits: usize) -> Circuit {
    let mut proofs = Vec::new();
    for a in 0..digits {
        if s >= a && s - a < digits {
            let b = s - a;
            proofs.push(vec![a, digits + b]);
        }
    }
    let mut c = build_dnf(&proofs);
    // Declare the full leaf space (2·digits), even for sums whose proofs don't
    // mention every digit, so gradients line up with the [p1; p2] vector.
    c.num_leaves = 2 * digits;
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dnf_wmc_is_exact() {
        // proofs {l0∧l1, l2}. If mutually exclusive: 0.5*0.4 + 0.3 = 0.5.
        let c = build_dnf(&[vec![0, 1], vec![2]]);
        let p = [0.5, 0.4, 0.3];
        assert!((c.wmc(&p) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn empty_and_certain() {
        assert_eq!(build_dnf(&[]).wmc(&[0.5]), 0.0); // no proof → false
        assert_eq!(build_dnf(&[vec![]]).wmc(&[0.5]), 1.0); // empty proof → true
    }

    #[test]
    fn sum_matches_direct_convolution() {
        let digits = 10;
        // random-ish distributions (normalized).
        let p1: Vec<f64> = (0..digits).map(|d| (d + 1) as f64).collect();
        let z1: f64 = p1.iter().sum();
        let p1: Vec<f64> = p1.iter().map(|x| x / z1).collect();
        let p2: Vec<f64> = (0..digits).map(|d| (digits - d) as f64).collect();
        let z2: f64 = p2.iter().sum();
        let p2: Vec<f64> = p2.iter().map(|x| x / z2).collect();
        let leaves: Vec<f64> = p1.iter().chain(&p2).copied().collect();

        for s in 0..(2 * digits - 1) {
            let got = sum_circuit(s, digits).wmc(&leaves);
            let want: f64 = (0..digits)
                .filter(|&a| s >= a && s - a < digits)
                .map(|a| p1[a] * p2[s - a])
                .sum();
            assert!((got - want).abs() < 1e-12, "s={s}: {got} vs {want}");
        }

        // Total probability over all sums is 1.
        let total: f64 = (0..(2 * digits - 1))
            .map(|s| sum_circuit(s, digits).wmc(&leaves))
            .sum();
        assert!((total - 1.0).abs() < 1e-9, "total {total}");
    }
}
