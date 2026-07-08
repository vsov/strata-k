<!-- The Python neural-symbolic recipe is the third end-to-end example:
     ../python/README.md (a GNN trained through grad_query). -->

# Workloads

Data-driven, end-to-end programs — bigger than the syntax examples one level
up, small enough to read whole. Each directory holds a program, its TSV data,
and the deterministic `gen.py` that produced the data. "Committed data == the
script's output" is a **checked fact, not a claim**: CI re-runs each generator
into a temp dir and compares the data sets **both ways** — every generated file
byte-equals its committed twin, and no committed data file survives that the
generator no longer produces
(`committed_workload_data_equals_generator_output`).

Every number quoted below is **pinned in CI**
(`crates/strata-cli/tests/workloads.rs`): the test runs the real binary and
asserts these exact lines, and a new workload directory fails the suite until
its output is pinned too.

## aml — the ownership screen (режим B)

60 firms, 62 ownership edges, 3 sanctioned entities — certain registry facts.
10 model flags with confidences — soft facts, loaded through the `neural`
predicate's trailing-probability column. Rules derive `control` (transitive
ownership), `investigate` with `Prov_k(4)` pedigrees (flagged directly, or
controls a flagged firm, or reached by a sanctioned holding), and `cleared` —
stratified negation over the *soft-derived* investigation set, its probability
the exact complement of the pedigree.

```
$ strata run examples/workloads/aml/aml.strata
0.9833902 :: investigate(g0_hold)  (lower bound, top-4)
1 :: cleared(indie0)  (lower bound, top-4)
0.14 :: cleared(indie7)  (lower bound, top-4)
...
  ∂/∂[0.85 :: flag(g4_op1)] = 0.110732  (→ model "aml_gnn")
```

The holding `g0_hold` reaches four flagged firms through its own subsidiaries
and cross-pyramid stakes — several proofs, shared evidence, one marginal. The
gradient block names the flag that would move the answer most: that is the
signal a host training loop backpropagates into the model. A firm reached by a
sanctioned holding is investigated with certainty, so its `cleared` tuple has
probability zero and is not derived at all.

## routing — all-pairs cheapest cost (Trop)

An 8×8 grid, 228 weighted links (both directions), plus express links out of
the hub `n0_0`. The same two transitive-closure rules as reachability, with
one word changed — `Bool` → `Trop` — compute every pair's cheapest cost
(⊕ = min, ⊗ = +). The full closure is 64×64 pairs; three plain `?route(...)`
queries filter the run down to the ones worth reading:

```
route(n0_0, n7_7) = 5     % the express link wins outright
route(n0_0, n4_4) = 2
route(n7_7, n0_0) = 44    % no express link back — full grid fare
```
