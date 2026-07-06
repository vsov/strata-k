# Strata/K

[![CI](https://github.com/vsov/strata-k/actions/workflows/ci.yml/badge.svg)](https://github.com/vsov/strata-k/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

A stratified, semiring-parameterized Datalog and its execution engine — the
symbolic core of a neuro-symbolic logic system. This repository is the CPU
reference stack: frontend, IR, checker, interpreter, and reference
implementations of the probabilistic (режим B) and answer-set layers. The GPU
engine and the knowledge-compilation / incremental optimizations are later
phases (see [ARCHITECTURE.md](ARCHITECTURE.md)).

> **Status.** The executable core runs end-to-end: `text → parse → check →
> Core-IR → interpret → result`. Positive Datalog, stratified negation, and
> aggregates over the **Bool** and **Trop** (tropical, min-plus) semirings;
> exact **probabilistic** queries (`?prob`, distribution semantics) with
> **gradients** (`?grad`, reverse-mode over the режим-B chain); **neural**
> predicates (model-sourced soft facts, differentiated back through `?grad`);
> **structural terms** (`@terms`, constructor terms via hash-consing, with a
> depth bound and a sound-but-incomplete status); **provenance annotations**
> (`Prov`/`Prov_k` — every derived fact carries its pedigree, compiled to a
> circuit for exact or declared-lower-bound marginals); and an **answer-set**
> (stable model) solver for `@asp` modules. Cross-checked against
> [Soufflé](https://souffle-lang.github.io/) (200 random programs per CI run;
> a 10k sweep is one env var away, see Correctness below) and fuzzed
> naive-vs-semi-naive over 10k random programs. **The whole shipped grammar
> executes**;
> the GPU backend runs beside this stack, validated bit-for-bit against it on
> CUDA hardware (`--features cuda`; hosted CI runs the CPU stub — see
> [Beyond the CPU pipeline](ARCHITECTURE.md#beyond-the-cpu-pipeline)).

## Quick start

```sh
cargo build
cargo run -p strata-cli -- run examples/tc.strata
```

Install the `strata` binary onto your PATH, or embed the engine as a library:

```sh
cargo install --path crates/strata-cli    # the `strata` binary
```

```toml
# Cargo.toml — the library facade (parse → check → run, queries, provenance,
# ASP, in-process neural models); see crates/strata-k/README.md
strata-k = { git = "https://github.com/vsov/strata-k" }
```

(The crates are not on crates.io yet; the git dependency and `cargo install
--path` are the supported channels.)

`examples/tc.strata` computes a transitive closure:

```
% transitive closure
domain node.
pred edge(node, node): Bool.
pred path(node, node): Bool.

path(X, Y) :- edge(X, Y).
path(X, Z) :- edge(X, Y), path(Y, Z).

edge(a, b).
edge(b, c).
edge(c, d).
```

```
$ strata run examples/tc.strata
edge(a, b)
edge(b, c)
edge(c, d)
path(a, b)
path(a, c)
path(a, d)
path(b, c)
path(b, d)
path(c, d)
```

## The language

Grammar priority: **optimized for LLMs to write**, human readability second.
Familiar Datalog `:-`, Prolog lexical convention (variables `Uppercase`,
constants `lowercase`), and **mandatory predicate signatures** so a typo becomes
a compile error, not a silently empty relation.

```
pred edge(node, node): Bool.          % a predicate signature: types + semiring
5 :: edge(a, b).                       % a tropical (weighted) fact
reach(X, Z) :- edge(X, Y), reach(Y, Z). % a recursive rule
unreach(X) :- node(X), not reach(X).    % stratified negation
outdeg(X, count<Y>) :- edge(X, Y).      % an aggregate
input edge from "edges.tsv".            % load facts from a Soufflé-compatible TSV
```

| Feature | Notes |
|---|---|
| Semirings | `Bool` (boolean Datalog), `Trop` (min-plus: shortest paths, Viterbi) |
| Weights | `Trop` weights are `i64` with a distinct `+∞` — comparisons are bit-exact |
| Negation | `not p(..)` — stratified in the deductive core; unstratified under `@asp` |
| Aggregates | `min`/`max`/`sum`/`count` (`prob_or` reserved); non-recursive |
| Probabilistic | `0.87 :: fact.` + `?prob q(..)` → exact marginals (distribution semantics) |
| Answer sets | `@asp.` module → stable models via a reference solver |
| EDB | inline facts, or `input p from "file.tsv"` |

```
$ strata run examples/prob.strata          # ?prob path(a, c)
0.625 :: path(a, c)

$ strata run examples/asp.strata           # @asp in/out choice over {x, y}
Answer 1: {in(x), in(y), node(x), node(y)}
Answer 2: {in(x), node(x), node(y), out(y)}
Answer 3: {in(y), node(x), node(y), out(x)}
Answer 4: {node(x), node(y), out(x), out(y)}
```

The full language is described in [docs/language.md](docs/language.md); the
formal grammar is in [docs/grammar.ebnf](docs/grammar.ebnf).

## CLI

```
strata check <file.strata> [--error-format=text|json]   # parse + type-check, no run
strata run   <file.strata> [--semi-naive]               # evaluate and print relations
strata fmt   <file.strata> [--check]                    # canonical formatter
strata ir    <file>        --to json|surface            # convert surface <-> JSON IR
```

Diagnostics carry a stable code (`E0xxx` front-end, `E1xxx` checker), a source
span, and (where possible) a machine-applicable fix:

```
$ strata check bad.strata
error[E1001]: predicate `edge` is used but never declared
  --> 2:1
   | path(X, X) :- edge(X, X).
   | ^^^^^^^^^^^^^^^^^^^^^^^^^
```

Exit codes: `0` ok · `1` diagnostics · `2` usage · `4` runtime fault.

## Two representations

The **High-IR JSON** document is the source of truth; the surface syntax is a
canonical projection of it (`strata ir` converts both ways, and `fmt` is
`parse → print`). An LLM can author either. The JSON Schema is published at
[`schema/high-ir.schema.json`](schema/high-ir.schema.json); the encoding
convention is in [`docs/ir-encoding.md`](docs/ir-encoding.md).

## Architecture

A Cargo workspace of ten crates, layered so the base has no sibling
dependencies:

```
strata-ir      IR data model (High-IR + Core-IR), symbol dictionary, diagnostics,
               JSON schema, tropical weight, hash-cons term table — the shared base
strata-front   lexer, parser (surface → High-IR), canonical printer, fmt, E0xxx diagnostics
strata-check   dependency graph, stratification, type/semiring checks (table 2.4),
               normalization High-IR → Core-IR, E1xxx diagnostics
strata-eval    the reference interpreter over Core-IR — naive T_P, semi-naive,
               exact probabilistic marginals + gradients (режим B), DRed; the oracle
strata-asp     the answer-set stack — reference solver, normalization, aspif,
               clasp embedding, unfounded-set verification
strata-gpu     the GPU execution engine (cuda-feature-gated; CPU stub without) —
               device-resident fixpoints, WCOJ, query planner, ASP grounding
strata-terms   structural-term machinery — interning, magic sets, points-to
strata-prob    provenance circuits — SDD-class WMC, gradients, top-k, MNIST-sum
strata-k       the library facade — embed the engine; Model trait (in-process neural)
strata-cli     the `strata` binary
```

`strata-front` and `strata-check` are siblings that both depend only on
`strata-ir`; the engine crates (`strata-gpu`, `strata-terms`, `strata-prob`)
sit beside `strata-eval` and are validated bit-for-bit against the reference
stack — for `strata-gpu` that validation needs CUDA hardware
(`cargo test -p strata-gpu --features cuda`; hosted CI runs the CPU stub, so
the badge does not cover the GPU differentials). See
[ARCHITECTURE.md](ARCHITECTURE.md) for the full picture and
[CONTRIBUTING.md](CONTRIBUTING.md) to build and test.

## Correctness

The reference interpreter is the oracle. It ships **two** engines — the
obviously-correct naive `T_P` fixpoint and a semi-naive delta engine — that are
cross-checked against each other, and against Soufflé for the Bool fragment:

```sh
cargo test                                     # unit + integration + corpus diffs
cargo test -p strata-eval --test fuzz          # naive == semi-naive over 10k random programs
cargo test -p strata-cli  --test souffle_diff  # our engine vs Soufflé (needs `souffle`)
STRATA_SOUFFLE_FUZZ_N=10000 cargo test -p strata-cli --test souffle_diff fuzz_bool_vs_souffle
```

Trop is validated against an independent shortest-path oracle rather than
Soufflé. If `souffle` is not installed, the differential tests skip cleanly.
Probabilistic queries are computed by exact possible-world enumeration (the
obviously-correct режим-B reference); answer sets by the Gelfond–Lifschitz
reduct — both the slow, exact oracles a compiled/GPU method must later match.

## The book

`book/` holds *Programs That Know Why* — a short book on logic programming in the
age of LLMs and the design of Strata/K, built with [mdBook](https://rust-lang.github.io/mdBook/).
📖 **Read it online: <https://vsov.github.io/strata-k/>**
**Work in progress.** Every runnable listing lives in `examples/book/` and runs
under the current `strata` CLI (CI-checked — see `crates/strata-cli/tests/book_listings.rs`). Text is licensed CC BY-SA 4.0; example code is
MIT/Apache like the rest of the repo.

```sh
mdbook serve book   # if you have mdBook installed
```

## Documentation

- [docs/language.md](docs/language.md) — the language reference (syntax + semantics)
- [ARCHITECTURE.md](ARCHITECTURE.md) — crate graph, two-level IR, evaluation engines
- [CONTRIBUTING.md](CONTRIBUTING.md) — build, test, and the differential-testing story
- [docs/grammar.ebnf](docs/grammar.ebnf) — the formal grammar
- [docs/ir-encoding.md](docs/ir-encoding.md) — the JSON IR encoding convention

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. The book text under `book/` is licensed
[CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/); its example code
follows the repo's MIT/Apache dual license.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
