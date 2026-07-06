# strata-terms

Structural terms and their program machinery — spec **Phase 3** (термы). Pure
CPU (the host stage); the device-side interning follow-up lives in
[`strata-gpu`](../strata-gpu). This is what lets Datalog carry function symbols
(lists, trees, records) without diverging.

The surface-language `@terms` feature (constructor terms in `.strata` programs,
run by `strata run`) uses the same hash-cons design via `strata-ir::terms` in
the reference interpreter; this crate is the standalone Phase-3 machinery —
subsumption, magic sets, and the points-to workload — that the engine work
builds on.

## What's here

| Module | Phase-3 task | What it does |
|---|---|---|
| `intern` | **host interning** + **depth bounds** | hash-cons ground terms → integer ids (structure sharing); reject terms past `max_depth` so term-building recursion terminates |
| `subsume` | **subsumption** | one-way matching — a more general atom subsumes a more specific one; `insert_maximal` keeps a fact store maximally general |
| `magic` | **magic sets** | Beeri–Ramakrishnan demand transformation (adornment + SIPS + magic predicates + seed) so bottom-up evaluation stays goal-directed |
| `pointsto` | **the exit** | Andersen points-to, field-sensitive via interned `field(obj, f)` terms; reports the interning-time fraction |

## Verification

`cargo test -p strata-terms` — 9 unit tests:

- **interning**: hash-cons shares structure; depth is tracked and bounded (depth
  4 rejected at `max_depth = 3`).
- **subsumption**: `p(X,a)` subsumes `p(b,a)`, `p(X,X)` subsumes `p(a,a)` not
  `p(a,b)`; the maximal store prunes subsumed facts.
- **magic sets**: on `ancestor`, the magic program gives the *same* query answers
  as the original while deriving strictly fewer facts (never touches the chain
  the query can't reach).
- **points-to**: field-sensitive flow (`a.f = c; d = b.f` ⇒ `d → o2`), transitive
  copies, multiple objects into one field.

## Exit — points-to at scale

`cargo run -p strata-terms --example pointsto --release` (size via
`STRATA_PT_VARS` / `STRATA_PT_FIELDS`). Default 300k objects × 4 fields →
**1.5M points-to pairs, 1.2M interned field terms** in ~3.8 s, with interning at
**~32% of wall-time** (~70% hash-cons hits) — the spec's "acceptable interning
fraction" success metric. The profile at which one moves interning onto the GPU
(`strata_gpu::intern_terms`).
