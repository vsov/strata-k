# strata-k

The **library facade** over the Strata/K reference stack — embed the engine
instead of shelling out to the CLI. One dependency, a small typed API:

```rust
use strata_k::{compile, eval, prob_query};

let mut program = compile(
    "pred edge(node, node): Bool.\n\
     pred path(node, node): Bool.\n\
     path(X, Y) :- edge(X, Y).\n\
     path(X, Z) :- edge(X, Y), path(Y, Z).\n\
     0.9 :: edge(a, b).\n\
     0.8 :: edge(b, c).\n",
)
.expect("checks");
let db = eval(&mut program).expect("runs"); // least fixpoint of the certain facts
let ans = prob_query(&mut program, "path", &[None, None]).expect("marginals");
assert_eq!(ans.len(), 3); // P(path(a,b))=0.9, P(path(b,c))=0.8, P(path(a,c))=0.72
```

(This snippet compiles and runs: the README is the crate's doc page, so
`cargo test` executes it as a doctest.)

## What's here

| API | What it does |
|---|---|
| `compile(src)` | parse + check: `text → High-IR → Core-IR`, typed diagnostics (E0xxx/E1xxx) |
| `eval(&mut checked)` | naive `T_P` least fixpoint (Bool, Trop, `@terms`) |
| `prob_query` / `grad_query` | exact marginals and `∂P/∂p_i` gradients (possible-world enumeration), take `&mut Checked` |
| `provenance(&mut checked)` | `Prov`/`Prov_k` capture — minimal proof DNFs; compile with `compile_exact`, count with `Circuit::wmc`/`grad` |
| `asp_models(src)` | stable models of an `@asp` program (declaration-checked) |
| `Model` + `attach_models` | **the in-process neural boundary**: a model object's forward pass supplies the soft facts — computed, not pasted — and gradients flow back to it; a second attach is a typed error, never a silent duplicate |
| `load_inputs(&program, &mut checked, base)` | resolve `input pred from "file"` declarations (TSV/CSV/JSON, columns typed by the declaration) — **atomic** (a mid-load failure commits nothing) and **once-only** (a reload is a typed error) |

Run the end-to-end neural example:

```sh
cargo run -p strata-k --example neural_inprocess
```

## Scope, honestly

This is a veneer: the semantics live in the reference crates
(`strata-front`/`strata-check`/`strata-eval`/`strata-asp`/`strata-prob`),
which remain independently usable, and every path here is the *reference*
implementation — exact, oracle-grade, not the fast engine. The GPU engine
(`strata-gpu`) is not behind this facade yet.
