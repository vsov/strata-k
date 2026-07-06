//! Exact compilation: a proof DNF → a deterministic/decomposable circuit.
//! [Prov, spec §2.1 stage 2]
//!
//! [`build_dnf`](crate::provenance::build_dnf) is exact only when the disjuncts
//! are mutually exclusive events; captured Datalog proofs share leaves, so its
//! plain `Or` over-counts (P(l0 ∨ l0∧l1) ≠ p0 + p0·p1). This module compiles the
//! same DNF by **Shannon expansion** — pick a variable `v`, split into the
//! `v=1` and `v=0` cofactors, recurse — producing decision nodes
//! `Or(And(Leaf v, hi), And(NegLeaf v, lo))` whose disjuncts are mutually
//! exclusive *by construction*, so [`Circuit::wmc`]/[`Circuit::grad`] are exact.
//! Cofactors are memoized (a DAG, OBDD-style); worst case is exponential — the
//! honest #P bill — but shared subproblems collapse.
//!
//! Literals are signed, aspif-style: `+(leaf+1)` means fact `leaf` present,
//! `-(leaf+1)` absent (the dual literal `x̄`). A proof containing both dies by
//! `x·x̄ = 0`; a proof that is a superset of another is absorbed.

use std::collections::{BTreeSet, HashMap};

use crate::circuit::{Builder, Circuit};

/// One proof: the set of signed leaf literals it conjoins.
type Proof = BTreeSet<i64>;
/// A DNF in absorption-minimal form (an antichain of proofs).
type Dnf = BTreeSet<Proof>;

/// Normalize raw proofs: drop contradictions (`x·x̄ = 0`), keep the
/// absorption-minimal antichain (a superset of another proof adds nothing:
/// P(A ∨ (A∧B)) = P(A)).
pub(crate) fn normalize(proofs: &[Vec<i64>]) -> Dnf {
    let mut sets: Vec<Proof> = proofs
        .iter()
        .map(|p| p.iter().copied().collect::<Proof>())
        .filter(|p| {
            debug_assert!(!p.contains(&0), "literal 0 is invalid (aspif convention)");
            p.iter().all(|&l| !p.contains(&-l))
        })
        .collect();
    // Shorter proofs first, so each kept proof only needs a subset check
    // against already-kept (shorter-or-equal) ones.
    sets.sort_by_key(|p| p.len());
    let mut min: Dnf = BTreeSet::new();
    for p in sets {
        if !min.iter().any(|kept| kept.is_subset(&p)) {
            min.insert(p);
        }
    }
    min
}

/// The default circuit-size budget. Shannon compilation is worst-case
/// exponential — the honest #P bill — so past this many nodes it stops with a
/// typed error instead of quietly eating the machine.
pub const MAX_CIRCUIT_NODES: usize = 1 << 20;

/// Compilation exceeded its node budget. The escape valves are declared, not
/// silent: `Prov_k` (top-k lower bound) or fewer soft facts in the slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetExceeded {
    pub max_nodes: usize,
}

impl std::fmt::Display for BudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "provenance circuit exceeded its {}-node budget (exact compilation is \
             #P-hard); annotate the predicate `Prov_k(k)` for a declared lower bound",
            self.max_nodes
        )
    }
}

impl std::error::Error for BudgetExceeded {}

/// Compile proofs into an exact circuit over `num_leaves` probabilistic facts,
/// under the default node budget. No proofs ⇒ `false`; an empty proof ⇒ `true`
/// (the tuple is certain).
pub fn compile_exact(proofs: &[Vec<i64>], num_leaves: usize) -> Result<Circuit, BudgetExceeded> {
    compile_exact_bounded(proofs, num_leaves, MAX_CIRCUIT_NODES)
}

/// [`compile_exact`] with an explicit node budget.
pub fn compile_exact_bounded(
    proofs: &[Vec<i64>],
    num_leaves: usize,
    max_nodes: usize,
) -> Result<Circuit, BudgetExceeded> {
    let dnf = normalize(proofs);
    let mut b = Builder::new();
    let mut memo: HashMap<Vec<Proof>, usize> = HashMap::new();
    let root = shannon(&dnf, &mut b, &mut memo, max_nodes)?;
    let mut c = b.finish(root);
    // Align the gradient vector with the caller's full leaf space even when the
    // proofs mention only some leaves (cf. `sum_circuit`).
    c.num_leaves = c.num_leaves.max(num_leaves);
    Ok(c)
}

fn shannon(
    dnf: &Dnf,
    b: &mut Builder,
    memo: &mut HashMap<Vec<Proof>, usize>,
    max_nodes: usize,
) -> Result<usize, BudgetExceeded> {
    if b.len() > max_nodes {
        return Err(BudgetExceeded { max_nodes });
    }
    if dnf.is_empty() {
        return Ok(b.fals());
    }
    if dnf.contains(&Proof::new()) {
        // Absorption left only the empty proof: certainly true.
        return Ok(b.tru());
    }
    let key: Vec<Proof> = dnf.iter().cloned().collect();
    if let Some(&n) = memo.get(&key) {
        return Ok(n);
    }

    // Pivot: the most frequent variable — smallest expected cofactors.
    let mut count: HashMap<i64, usize> = HashMap::new();
    for p in dnf {
        for &l in p {
            *count.entry(l.abs()).or_insert(0) += 1;
        }
    }
    let (&v, _) = count
        .iter()
        .max_by_key(|(var, n)| (**n, -**var)) // deterministic tie-break
        .expect("non-empty proofs have variables");

    // Cofactor v=1: proofs demanding ¬v die; the +v literal is discharged.
    let hi: Dnf = restrict(dnf, v, true);
    // Cofactor v=0: proofs demanding v die; the -v literal is discharged.
    let lo: Dnf = restrict(dnf, v, false);

    let hi_n = shannon(&hi, b, memo, max_nodes)?;
    let lo_n = shannon(&lo, b, memo, max_nodes)?;
    let leaf = (v - 1) as usize;
    let pos = b.leaf(leaf);
    let neg = b.neg_leaf(leaf);
    let and_hi = b.and(vec![pos, hi_n]);
    let and_lo = b.and(vec![neg, lo_n]);
    let node = b.or(vec![and_hi, and_lo]);
    memo.insert(key, node);
    Ok(node)
}

/// The cofactor of `dnf` under `var = value`, re-minimized by absorption.
fn restrict(dnf: &Dnf, var: i64, value: bool) -> Dnf {
    let (dies, discharged) = if value { (-var, var) } else { (var, -var) };
    let mut out: Dnf = BTreeSet::new();
    let survivors: Vec<Proof> = dnf
        .iter()
        .filter(|p| !p.contains(&dies))
        .map(|p| {
            let mut q = p.clone();
            q.remove(&discharged);
            q
        })
        .collect();
    for q in survivors {
        if !out.iter().any(|kept| kept.is_subset(&q)) {
            out.retain(|kept| !q.is_subset(kept));
            out.insert(q);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Brute-force P(∨ proofs) by enumerating all 2^n worlds — the oracle.
    fn brute(proofs: &[Vec<i64>], p: &[f64]) -> f64 {
        let n = p.len();
        let mut total = 0.0;
        for mask in 0u32..(1u32 << n) {
            let w: f64 = (0..n)
                .map(|i| {
                    if mask & (1 << i) != 0 {
                        p[i]
                    } else {
                        1.0 - p[i]
                    }
                })
                .product();
            let sat = proofs.iter().any(|proof| {
                proof.iter().all(|&l| {
                    let i = (l.abs() - 1) as usize;
                    let present = mask & (1 << i) != 0;
                    if l > 0 {
                        present
                    } else {
                        !present
                    }
                })
            });
            if sat {
                total += w;
            }
        }
        total
    }

    #[test]
    fn shared_leaves_are_exact_where_build_dnf_overcounts() {
        // {l0}, {l0∧l1}, {l1}: exact P = P(l0 ∨ l1) = 0.75 at p = 0.5/0.5.
        // A plain Or would give 0.5 + 0.25 + 0.5 = 1.25.
        let proofs = vec![vec![1], vec![1, 2], vec![2]];
        let p = [0.5, 0.5];
        let c = compile_exact(&proofs, 2).unwrap();
        assert!((c.wmc(&p) - 0.75).abs() < 1e-12, "got {}", c.wmc(&p));
        assert!((c.wmc(&p) - brute(&proofs, &p)).abs() < 1e-12);
    }

    #[test]
    fn correlated_two_route_reachability() {
        // The prob.rs example: direct {e1} or via {e2,e3}; P = 1-(1-.5)(1-.25).
        let proofs = vec![vec![1], vec![2, 3]];
        let p = [0.5, 0.5, 0.5];
        let c = compile_exact(&proofs, 3).unwrap();
        assert!((c.wmc(&p) - 0.625).abs() < 1e-12);
    }

    #[test]
    fn matches_brute_force_on_random_dnfs() {
        // Deterministic pseudo-random sweep: many shapes, up to 6 leaves.
        let mut seed = 0x9e3779b97f4a7c15u64;
        let mut rng = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for case in 0..200 {
            let n = 2 + (rng() % 5) as usize; // 2..=6 leaves
            let n_proofs = 1 + (rng() % 5) as usize;
            let proofs: Vec<Vec<i64>> = (0..n_proofs)
                .map(|_| {
                    let len = 1 + (rng() % 3) as usize;
                    (0..len)
                        .map(|_| {
                            let v = 1 + (rng() % n as u64) as i64;
                            if rng() % 4 == 0 {
                                -v
                            } else {
                                v
                            }
                        })
                        .collect()
                })
                .collect();
            let p: Vec<f64> = (0..n).map(|_| (rng() % 1000) as f64 / 1000.0).collect();
            let c = compile_exact(&proofs, n).unwrap();
            let (got, grad) = c.grad(&p);
            let want = brute(&proofs, &p);
            assert!(
                (got - want).abs() < 1e-9,
                "case {case}: proofs {proofs:?} p {p:?}: {got} vs {want}"
            );
            // Gradient against central finite differences on the brute force.
            let eps = 1e-6;
            for i in 0..n {
                let mut pp = p.clone();
                pp[i] += eps;
                let mut pm = p.clone();
                pm[i] -= eps;
                let fd = (brute(&proofs, &pp) - brute(&proofs, &pm)) / (2.0 * eps);
                assert!(
                    (grad[i] - fd).abs() < 1e-4,
                    "case {case} leaf {i}: {} vs {fd}",
                    grad[i]
                );
            }
        }
    }

    #[test]
    fn contradiction_and_absorption() {
        // {l0 ∧ ¬l0} is dropped (x·x̄=0); {l1} absorbs {l1 ∧ l2}.
        let proofs = vec![vec![1, -1], vec![2], vec![2, 3]];
        let dnf = normalize(&proofs);
        assert_eq!(dnf.len(), 1);
        let c = compile_exact(&proofs, 3).unwrap();
        assert!((c.wmc(&[0.9, 0.4, 0.7]) - 0.4).abs() < 1e-12);
    }

    #[test]
    fn dual_literals_negation() {
        // "a and not b": P = p_a · (1 - p_b).
        let proofs = vec![vec![1, -2]];
        let c = compile_exact(&proofs, 2).unwrap();
        let p = [0.8, 0.3];
        assert!((c.wmc(&p) - 0.8 * 0.7).abs() < 1e-12);
        let (_, g) = c.grad(&p);
        assert!((g[0] - 0.7).abs() < 1e-12);
        assert!((g[1] + 0.8).abs() < 1e-12); // ∂/∂p_b = -p_a
    }

    #[test]
    fn node_budget_trips_deterministically() {
        // A parity-ish DNF over many variables blows up Shannon compilation;
        // a tiny budget must produce the typed error, not a hang or OOM.
        let proofs: Vec<Vec<i64>> = (1..=16)
            .flat_map(|i| ((i + 1)..=16).map(move |j| vec![i as i64, -(j as i64)]))
            .collect();
        let err = compile_exact_bounded(&proofs, 16, 8).unwrap_err();
        assert_eq!(err.max_nodes, 8);
        // The same DNF fits comfortably in the default budget.
        assert!(compile_exact(&proofs, 16).is_ok());
    }

    #[test]
    fn empty_and_certain() {
        assert_eq!(compile_exact(&[], 1).unwrap().wmc(&[0.5]), 0.0);
        assert_eq!(compile_exact(&[vec![]], 1).unwrap().wmc(&[0.5]), 1.0);
        // A certain proof absorbs every soft one.
        assert_eq!(
            compile_exact(&[vec![1], vec![]], 1).unwrap().wmc(&[0.5]),
            1.0
        );
    }
}
