# strata-prob

Provenance and the soft pipeline — spec **Phase 4** (режим B). Pure CPU. The
principle: *a probability is not a weight*. A naive semiring convolution
double-counts a fact shared by two derivations; режим B captures each tuple's
provenance and counts every fact once.

## What's here

| Module | Phase-4 task | What it does |
|---|---|---|
| `circuit` | **SDD-class circuit + WMC + gradients** | decomposable-AND / deterministic-OR circuit (with `NegLeaf` dual literals); `wmc` (exact marginal), `grad` (∂WMC/∂pᵢ — the autodiff bridge) |
| `provenance` | **chain capture** | derivations → OR-of-ANDs circuit (`build_dnf`, exact for mutually exclusive disjuncts); `sum_circuit` for MNIST-sum |
| `compile` | **exact compilation** (`Prov`) | proof DNF with *shared* leaves → deterministic circuit by Shannon expansion (memoized, OBDD-style); exact where a plain OR over-counts |
| `topk` | **top-k** (`Prov_k`) | the diff-top-k proofs; `topk_circuit` = exact WMC of the *union* of the kept proofs — a guaranteed lower bound (a plain sum is not, once proofs overlap) |
| `mnist_sum` | **the exit** | learn digits from sum-only supervision, with a **compilation cache** across epochs |

This crate is the engine behind the surface language's `Prov`/`Prov_k`
annotations: `strata-eval::provenance` captures the proofs, `compile`/`topk`
compile them, `circuit` counts and differentiates.

## Verification

`cargo test -p strata-prob` — 19 unit tests:

- **circuit**: WMC of an AND/OR circuit; gradient matches finite differences
  (incl. an AND with a zero child); `NegLeaf` gradients.
- **provenance**: DNF WMC is exact; `sum_circuit` equals the direct convolution
  `Σ p1[a]·p2[s−a]` for every sum, and the total over all sums is 1.
- **compile**: 200 random DNFs (shared leaves, dual literals) — WMC and
  gradients against brute-force world enumeration; contradiction (`x·x̄`) and
  absorption handling.
- **topk**: picks the highest-probability proofs; the signed union-WMC is a
  lower bound where a sum exceeds 1, monotone in k, exact at full k;
  selection is deterministic under input permutation.
- **mnist_sum**: learns single-digit classification from sum labels to >0.9.

## Exit — MNIST-sum parity

`cargo run -p strata-prob --example mnist_sum --release`

A linear digit classifier is trained from **sum labels only**, end-to-end
through the differentiable provenance circuit. Single-digit test accuracy climbs
from chance (0.10) to **~100%** — it learned to classify digits it was never
directly told, the DeepProbLog / Scallop result ("accuracy parity"). The 2·D−1
sum circuits are compiled once and reused every example × epoch (19 built,
75,000 reuses at the default size).

## Notes

- The circuit is an SDD/d-DNNF-*class* structure (decomposable AND, deterministic
  OR) with exact WMC; wiring an external SDD compiler and a real PyTorch binding
  (the gradient interface is already here) is engineering breadth.
- `strata-eval::prob` is the режим-B *reference* (exact possible-world
  enumeration); this crate is the provenance→circuit→WMC→gradient pipeline that
  makes it differentiable and cacheable.
