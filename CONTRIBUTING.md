# Contributing to Strata/K

Thanks for your interest. Strata/K is an early-phase project — a CPU reference
stack for a semiring-parameterized Datalog. This document covers how to build,
test, and where things live. For the design see
[ARCHITECTURE.md](ARCHITECTURE.md); for the language see
[docs/language.md](docs/language.md).

## Prerequisites

- **Rust 1.82+** (the workspace MSRV). Install via [rustup](https://rustup.rs/);
  a `rust-toolchain.toml` pins the toolchain.
- **Soufflé** (optional) — only the differential Bool tests use it. Everything
  else builds and tests without it. See
  [souffle-lang.github.io](https://souffle-lang.github.io/).

## Build and run

```sh
cargo build                                   # build the workspace
cargo run -p strata-cli -- run examples/tc.strata
cargo run -p strata-cli -- check examples/prob.strata
```

The binary is `strata`; its subcommands (`check`, `run`, `fmt`, `ir`) are
documented in the [README](README.md#cli).

## Test

```sh
cargo test                                    # unit + integration across the workspace
cargo test -p strata-eval --test fuzz         # naive == semi-naive over 10k random programs
cargo test -p strata-cli  --test souffle_diff # our Bool engine vs Soufflé (needs `souffle`)
```

The differential fuzzer's program count is tunable:

```sh
STRATA_SOUFFLE_FUZZ_N=10000 cargo test -p strata-cli --test souffle_diff fuzz_bool_vs_souffle
```

If `souffle` is not on `PATH`, the Soufflé jobs skip cleanly rather than fail.

## The bar for a change (what CI enforces)

CI runs the same four gates; run them locally before pushing:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
```

`fmt`-clean, `clippy` with `-D warnings`, green build, green tests. Clippy
warnings are errors — do not merge over them.

## Correctness is the product

The reference interpreter is an *oracle*: its job is to be obviously correct, not
fast. When you touch an engine, the naive and semi-naive results must stay
bit-identical, and (for the Bool fragment) must still match Soufflé. If you add a
faster path, it must be cross-checked against the slow one, not replace it. New
evaluation behavior should come with a differential or property test, not just an
example.

## Where things live

```
crates/strata-ir      IR data model, dictionary, diagnostics, JSON schema, weight
crates/strata-front   lexer, parser, printer, fmt      (E0xxx diagnostics)
crates/strata-check   graph, stratification, checks, normalization (E1xxx)
crates/strata-eval    naive / semi-naive / probabilistic engines + oracle
crates/strata-asp     answer-set (stable model) solver
crates/strata-cli     the `strata` binary
docs/                 language reference, grammar, IR encoding
schema/               the published High-IR JSON Schema
examples/             runnable programs (examples/book/ back the book listings)
book/                 "Programs That Know Why" (mdBook; work in progress)
```

Adding a diagnostic: allocate a code in the owning crate's `diagnostics.rs`
`codes` module (front owns `E0xxx`, check owns `E1xxx`), add it to that module's
`ALL` table (a conformance test checks the registry), and emit it with a span and
— where you can — a machine-applicable fix.

Regenerating the JSON schema after changing High-IR:

```sh
cargo run -p strata-ir --example gen_schema > schema/high-ir.schema.json
```

## Licensing of contributions

Unless you state otherwise, contributions are dual-licensed under MIT OR
Apache-2.0, matching the repository. Book text under `book/` is CC BY-SA 4.0.
