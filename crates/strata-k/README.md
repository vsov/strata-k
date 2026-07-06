# strata-k

The **library facade** over the Strata/K reference stack — embed the engine
instead of shelling out to the CLI. One dependency, a small typed API:

```rust
use strata_k::{compile, eval, prob_query};

let mut program = compile("pred edge(node, node): Bool. …")?;
let db = eval(&mut program)?;                       // least fixpoint (Bool/Trop/@terms)
let ans = prob_query(&program, "path", &pattern)?;  // exact режим-B marginals
```

## What's here

| API | What it does |
|---|---|
| `compile(src)` | parse + check: `text → High-IR → Core-IR`, typed diagnostics (E0xxx/E1xxx) |
| `eval(&mut checked)` | naive `T_P` least fixpoint (Bool, Trop, `@terms`) |
| `prob_query` / `grad_query` | exact marginals and `∂P/∂p_i` gradients (possible-world enumeration) |
| `provenance(&checked)` | `Prov`/`Prov_k` capture — minimal proof DNFs; compile with `compile_exact`, count with `Circuit::wmc`/`grad` |
| `asp_models(src)` | stable models of an `@asp` program (declaration-checked) |
| `Model` + `attach_models` | **the in-process neural boundary**: a model object's forward pass supplies the soft facts — computed, not pasted — and gradients flow back to it |

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
