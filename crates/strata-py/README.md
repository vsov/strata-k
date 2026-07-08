# strata-k (Python)

The Python bridge to the [Strata/K](https://github.com/vsov/strata-k) engine —
stratified, semiring-parameterized Datalog with probabilistic queries,
gradients, and provenance.

```bash
pip install maturin
pip install ./crates/strata-py        # or: maturin develop, from this directory
```

```python
import strata_k

p = strata_k.compile("""
    domain node.
    pred edge(node, node): Bool.
    pred path(node, node): Prov_k(4).
    path(X, Y) :- edge(X, Y).
    path(X, Z) :- edge(X, Y), path(Y, Z).
    0.9 :: edge(a, b).
    0.8 :: edge(b, c).
""")
print(p.prob_query("path", ["a", "c"]))   # exact marginal: 0.72
print(p.grad_query("path", ["a", "c"]))   # + ∂P/∂p_i per soft fact
print(p.provenance())                     # proof DNFs, signed literals
```

(Recursive `Prov` is refused exactly — E1008; `Prov_k(k)` keeps the top-k
proofs, whose compiled union is a guaranteed lower bound.)

- **Values**: symbols ⇄ `str`, integers ⇄ `int`; compound `@terms` values come
  out rendered (`"f(a, 3)"`). Trop rows carry a trailing weight.
- **Models in-process**: `p.attach_models({"name": callable})` — the callable's
  `[(pred, args, prob), ...]` become the `neural ... from model "name"` facts,
  and `grad_query` gradients align with `p.prob_facts()`.
- **External compilers**: `p.provenance()` hands out raw proof DNFs;
  `strata_k.wmc(proofs, probs)` / `wmc_grad` are the exact reference to diff
  against (the test suite diffs PySDD this way).

The semantics live in the Rust crates; this module is a thin, typed veneer.
See `docs/language.md` at the repository root for the language contract.
