# strata-gpu

The GPU execution engine for Strata/K — spec **Phase 1** (режим A: idempotent
semirings on the GPU), **Phase 2** (worst-case-optimal joins) and the **Phase 5**
GPU grounding-simplification for ASP. CUDA kernels
are compiled at runtime with `nvrtc` and
launched from Rust via [`cudarc`]. The crate is `cuda`-feature-gated: without the
feature it builds to a stub (`GpuError::NotBuilt`) so the rest of the workspace
compiles on machines with no CUDA; with `--features cuda` on an NVIDIA box it
runs the real engine.

The CPU reference interpreter (`strata-eval`) is the **bit-exact oracle**
(constitution invariant I5): every GPU result is diffed against an independent
CPU computation.

## What runs

| Component | Status |
|---|---|
| Columnar relations (`(src,dst)` u32 columns, `i64` weight) | ✅ |
| Semi-naive (delta) evaluation | ✅ |
| Two-phase join allocation (count → prefix-sum → emit) | ✅ |
| GPU relational join (sorted-column, binary-search probe) | ✅ |
| Device sort — **LSD radix** (u64), O(n) | ✅ |
| Device dedup (Bool `⊕`) — flag + scan + compact | ✅ |
| Device min-by-key (Trop `⊕`) — kv-sort + segmented min | ✅ |
| **Bool** semiring → transitive closure | ✅ |
| **Trop** (min-plus) semiring → shortest paths | ✅ |
| Bool + Trop closures device-resident (no host round-trips) | ✅ |
| **TC + SSSP on 10⁸-edge graphs, bit-exact vs reference** | ✅ |
| **WCOJ (leapfrog triejoin)**: triangle + 4-clique counting | ✅ (Phase 2) |
| Skew (power-law): count without OOM where binary plans explode | ✅ (Phase 2) |
| **Hypertree decomposition** (GYO) + **cost-based optimizer** | ✅ (Phase 2) |
| Per-superstep re-optimization (re-plan on fresh stats) | ✅ (Phase 2) |
| **ASP grounding-simplification** (§5.2 simplify-before-transfer) | ✅ (Phase 5) |

## Verification

All on `datasci.bo.ath` (Ada / RTX-4090-class, CUDA 11.8), every result
bit-exact against an independent oracle:

- **Primitives** (in-crate unit tests): radix sort vs host sort over the full
  64-bit range; unique / min-by-key vs host across 256-block boundaries and
  heavy duplication.
- **Bool TC** (`tests/tc_diff.rs`): a set-fixpoint oracle over 200 random graphs
  + a 400-edge chain (400 deep semi-naive rounds).
- **Trop SSSP** (`tests/trop_diff.rs`): Floyd-Warshall over 200 random
  non-negative-weighted graphs + a weighted 300-chain.
- **Scale** (`examples/bench.rs`, size-tunable via `STRATA_BENCH_K`/`_C`), both
  semirings device-resident and bit-exact on the Ada card:
  - 10M edges → 30M closure: Bool ~1.08 s (27.7 M tuples/s), Trop ~2.07 s (14.5 M/s)
  - **100M edges → 150M closure**: Bool ~3.2 s (47 M tuples/s), Trop ~7.9 s (19 M/s)
  - Bool alone reaches 100M edges → 300M closure (~9.1 s) — the Trop closure of
    that size overflows 24 GB (its kv-sort still pads to a power of two).
  Throughput rises with size (better GPU saturation).
- **GPU vs the reference engine** (`examples/race.rs`): the *same* TC/SSSP
  program run through both the GPU kernels and `strata-eval`'s semi-naive CPU
  interpreter, sizes asserted equal on both sides so no ratio is ever quoted
  off a wrong answer. On a 100k-edge chain-union (300k closure): Bool GPU
  ~0.20 s vs CPU ~415 s, Trop GPU ~0.013 s vs CPU ~427 s. **Read this as why
  the GPU backend exists, not as a portable "N× faster" number:** the
  reference interpreter is deliberately the *obviously-correct* oracle (I5),
  not an optimized engine — its join is a naive nested loop, so a closure this
  size already takes minutes on the CPU while the GPU finishes in
  milliseconds. An optimized CPU Datalog (Soufflé) would close most of that
  gap; the honest claim is "режим A needs a device backend to stay
  interactive at scale," not a headline multiplier.
- **WCOJ / skew** (`examples/tri.rs`, `tests/wcoj_diff.rs`): triangle + 4-clique
  counts bit-exact vs O(n³)/O(n⁴) brute force + complete-graph closed forms. On a
  9.6M-edge power-law graph the binary-join intermediate is Σd² = 3.1×10¹²
  two-paths (~50 TB → OOM), while WCOJ counts 12,000,560 triangles in ~0.54 s
  with a ~115 MB working set — **326,988× smaller**, the Phase-2 exit.
- **ASP grounding-simplification** (`tests/asp_diff.rs`, spec §5.2): the
  simplify-before-transfer pass — drop rules with a definitely-false body,
  substitute certain/impossible atoms out of surviving bodies, compact — bit-exact
  vs the CPU reference on 200 random ground programs. On 4M instantiated rules
  the device filter runs ~0.039 s (**103 M rules/s**) vs ~0.37 s on the CPU — a
  **~9.5× speedup** on the grounding step (the Phase-5 exit's speedup metric).
  The `poss`/`cert` fixpoints and the dedup/subsumption pass stay on the CPU;
  the aspif emission, clasp embedding, normalization and unfounded-set verifier
  live in [`strata-asp`](../strata-asp).

```sh
# on the GPU box:
export CUDA_ROOT=/usr/local/cuda-11.8
export LD_LIBRARY_PATH=/usr/local/cuda-11.8/targets/x86_64-linux/lib:$LD_LIBRARY_PATH
cargo test  -p strata-gpu --features cuda
cargo run   -p strata-gpu --example bench --features cuda --release
```

## Design notes

- A `(src,dst)` pair encodes into one `u64` key (`src << 32 | dst`), so sorting
  keys yields the canonical relation order the join relies on, and unique/min on
  keys implements each semiring's `⊕`.
- The exclusive scan (block Hillis-Steele in shared memory + a host scan of the
  block sums) is the shared building block behind radix sort, dedup, and
  min-by-key.
- This crate is the only one to opt out of the workspace `unsafe_code = "forbid"`
  — `cudarc` kernel launches are inherently `unsafe`.

## Remaining (performance hardening)

The Phase-1 exit is met: **TC (Bool) and SSSP (Trop) run on 10⁸-edge graphs,
bit-exact against a known-closure reference**, both fully device-resident. What
remains is tuning, not functionality:

- **Trop key-value radix** — the min path's kv-sort still pads to a power of two,
  which is what makes a 3×10⁸-tuple closure overflow 24 GB. An LSD kv-radix
  (no padding) would let Trop match Bool's memory footprint at 10⁸ edges.
- **Radix hash-join** — the join is currently a correct sorted-column
  binary-search (sort-merge) probe; a radix hash-join is the spec's alternative.
- **Throughput** — ~30–50 M closure-tuples/s on one Ada card is a reasonable
  reference rate; closing the gap to hand-tuned published systems (fused
  kernels, better occupancy) is ongoing.

All named Phase-2 tasks are implemented and verified: WCOJ (the leapfrog
kernels), the cost-based optimizer and hypertree decomposition (`crate::plan`,
CPU query-planning that produces the plan the kernels run), and per-superstep
re-optimization (re-planning on fresh per-round statistics). The exit —
triangle/clique counting without OOM where a binary plan blows up — is met.

What remains is engineering breadth, not a missing Phase-2 task: a **general
GPU plan-executor** that runs an arbitrary `Plan` (any hypertree bag) on the
device — today the leapfrog kernels are specialized to the triangle and K4
bodies, while the planner already handles arbitrary conjunctive queries.

[`cudarc`]: https://crates.io/crates/cudarc
