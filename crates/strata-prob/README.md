# strata-prob

Provenance and the soft pipeline — spec **Phase 4** (режим B). Pure CPU. The
principle: *a probability is not a weight*. A naive semiring convolution
double-counts a fact shared by two derivations; режим B captures each tuple's
provenance and counts every fact once.

## What's here

| Module | Phase-4 task | What it does |
|---|---|---|
| `circuit` | **SDD-class circuit + WMC + gradients** | decomposable-AND / deterministic-OR circuit; `wmc` (exact marginal), `grad` (∂WMC/∂pᵢ — the autodiff bridge) |
| `provenance` | **chain capture** | derivations → OR-of-ANDs circuit (`build_dnf`); `sum_circuit` for MNIST-sum |
| `topk` | **top-k** | the diff-top-k proofs — sparse differentiable surrogate for exact WMC |
| `mnist_sum` | **the exit** | learn digits from sum-only supervision, with a **compilation cache** across epochs |

## Verification

`cargo test -p strata-prob` — 9 unit tests:

- **circuit**: WMC of an AND/OR circuit; gradient matches finite differences
  (incl. an AND with a zero child).
- **provenance**: DNF WMC is exact; `sum_circuit` equals the direct convolution
  `Σ p1[a]·p2[s−a]` for every sum, and the total over all sums is 1.
- **topk**: picks the highest-probability proofs; full-k equals the exact total.
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
