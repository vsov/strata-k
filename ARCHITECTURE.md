# Architecture

Strata/K is a Cargo workspace. This document maps the crates, the two-level IR,
and the evaluation engines, and explains the design invariants that hold the
whole thing together. For the user-facing language see
[docs/language.md](docs/language.md); to build and test see
[CONTRIBUTING.md](CONTRIBUTING.md).

## The pipeline

```
text ──parse──▶ High-IR ──check──▶ Core-IR ──interpret──▶ result
      (front)   (JSON,      (check)  (internal,  (eval / asp)
                 public)             stratified)
```

- **Surface text** and **High-IR JSON** are two forms of the same program.
  High-IR is the canonical source of truth; the surface syntax is a projection.
- **check** validates the program (declarations, safety, stratification,
  semiring rules) and *normalizes* it into Core-IR: a lower-level, stratified,
  slot-indexed representation the engines consume.
- **eval** runs Core-IR to a least fixpoint; **asp** solves `@asp` programs.

## Crates

The workspace is layered so the base crate has no sibling dependencies:

```
strata-ir      IR data model (High-IR + Core-IR), symbol dictionary, diagnostics,
               JSON schema, tropical weight — the shared base (no sibling deps)
strata-front   lexer (logos), recursive-descent parser (surface → High-IR),
               canonical printer, fmt; owns the E0xxx diagnostic range
strata-check   dependency graph, stratification, type/semiring checks (table 2.4),
               normalization High-IR → Core-IR; owns the E1xxx diagnostic range
strata-eval    the reference interpreter over Core-IR — naive T_P, semi-naive,
               and exact probabilistic marginals (режим B); the differential oracle
strata-asp     the reference answer-set solver (grounding + stable models)
strata-cli     the `strata` binary — ties the crates into the end-to-end path
```

Dependency edges point only *down*: `strata-front` and `strata-check` are
siblings that each depend only on `strata-ir`; `strata-eval` and `strata-asp`
depend on `strata-ir`; `strata-cli` depends on all of them. Shared data types
(the symbol dictionary, `GroundVal`/`GroundFact`, the `Diagnostics` collector)
live in `strata-ir` precisely so the siblings can share them without depending on
each other.

The workspace forbids `unsafe` by default (`unsafe_code = "forbid"`), relaxed
per-crate only when the device-side GPU work arrives. MSRV is **Rust 1.82**.

## The two-level IR

- **High-IR** (`strata-ir::high`) is the public, LLM-writable model. It is what
  the JSON Schema describes and what `strata ir` reads and writes. Items carry
  *trivia* (comments, blank lines) and *spans* (source locations) that are
  preserved by the formatter but excluded from semantic equality — two programs
  equal up to trivia are the same program.
- **Core-IR** (`strata-ir::core`) is the internal, normalized form: predicates
  carry an explicit stratum, rules carry variable slots and a resolved semiring,
  terms are slot-indexed. It is deliberately narrow — the surface's conveniences
  are compiled away so the engines (and a future GPU backend) see one flat,
  stratified shape.

The JSON encoding is stable and documented in
[docs/ir-encoding.md](docs/ir-encoding.md): payload enums are adjacently tagged
`{ "kind": ..., "data": ... }`, unit enums are snake_case strings, and a
tropical weight is an integer or the string `"inf"`.

## Evaluation engines

`strata-eval` ships more than one engine on purpose — correctness is asserted by
cross-checking them, not assumed.

- **Naive `T_P`** (`naive.rs`): the obviously-correct least-fixpoint. Apply every
  rule over the whole database until nothing new appears. Handles stratified
  negation and aggregates by evaluating strata in order (a negated/aggregated
  predicate is fully computed in a lower stratum before it is read).
- **Semi-naive** (`seminaive.rs`): the delta engine. Each recursive round joins
  using at least one tuple that changed in the previous round, so already-derived
  tuples are not recomputed. It must produce a database **bit-identical** to
  naive on every program — this is the core Phase-0 correctness signal and the
  algorithm the GPU backend will port.
- **Probabilistic (режим B)** (`prob.rs`): exact marginals by enumerating all
  `2^n` possible worlds over the probabilistic facts. Exponential and exact — the
  reference a compiled/top-k method must match. Bool deduction only.

`strata-asp` is the answer-set counterpart: a naive grounder over the Herbrand
universe plus a Gelfond–Lifschitz reduct that guesses only the negated atoms
(`2^|N| ≪ 2^|HB|`), confirming each candidate is a stable model.

## Correctness invariants

The reference interpreter is the oracle. Three overlapping checks defend it:

1. **naive == semi-naive**, fuzzed over 10k random programs
   (`cargo test -p strata-eval --test fuzz`).
2. **our Bool engine == Soufflé**, a differential harness that translates Core-IR
   to a Soufflé `.dl` program and compares results
   (`cargo test -p strata-cli --test souffle_diff`, needs `souffle`; skips
   cleanly if absent). `Trop` is checked against an independent shortest-path
   oracle instead.
3. The probabilistic and ASP engines are themselves the slow, obviously-correct
   references (exact enumeration; reduct) that future fast/compiled/GPU methods
   must reproduce bit-for-bit.

## Roadmap (designed, not yet built)

Everything in this repository is the CPU reference stack. The following are
designed and reserved in the IR and grammar — they parse into valid IR and
return a stable *"not implemented in Phase 0"* diagnostic — but are not executed
here:

- **GPU execution engine** — a columnar, semi-naive fixpoint on the GPU,
  consuming the same Core-IR beside `strata-eval`.
- **Knowledge compilation** for режим B (SDD/WMC, top-k proofs) to replace exact
  possible-world enumeration.
- **Incremental (differential) evaluation** — update conclusions as facts arrive,
  without recomputing.
- **`Prov` / `Prov_k` provenance**, **`neural` predicates**, **`@terms`**
  (structural terms), and **`?grad`** (gradient) queries.

The point of shipping the slow references first is that each fast engine has a
bit-exact oracle to be tested against from day one.
