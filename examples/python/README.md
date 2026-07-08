# Python recipe — a neural-symbolic bridge

The third end-to-end recipe (alongside the AML compliance graph and Trop
routing in [`examples/workloads/`](../workloads/README.md)): a neural network
trained **through** the logic layer, with the exact gradient flowing from the
engine back into the model's weights.

`train_gnn.py` is the runnable version; this is the walk-through.

## The problem

A tiny message-passing GNN scores candidate edges of a 4-node graph. We want
the *derived* relation to come out right — `c` reachable from `a`, `d` **not**
reachable from `a` — without ever labelling individual edges. The catch is
combinatorial: the edges that make `a→c` hold and the edges that make `a→d`
hold overlap, so the model has to learn *which edges to believe*, not a
per-edge target.

## The bridge, in four moves

**1. Declare the edges neural.** The program treats edge existence as soft
facts supplied by a model:

```
domain node.
neural edge(node, node) from model "gnn".
pred reach(node, node): Bool.
reach(X, Y) :- edge(X, Y).
reach(X, Z) :- edge(X, Y), reach(Y, Z).
```

**2. Attach the model in-process.** The GNN's forward pass produces a
probability per candidate edge; `attach_models` wires those in as the
`"gnn"` model's soft facts. Gradient positions follow `prob_facts()`, which is
exactly the candidate order — the code asserts this alignment rather than
trusting it:

```python
program = strata_k.compile(SRC)
program.attach_models({"gnn": lambda: [("edge", e, probs[i]) for i, e in enumerate(CANDIDATES)]})
assert [f[1] for f in program.prob_facts()] == CANDIDATES
((_, prob, grads),) = program.grad_query("reach", ["a", "c"])
```

`grad_query` returns the exact marginal `P(reach(a, c))` **and** `∂P/∂p(edge)`
for every soft edge — reverse-mode over the possible-world chain, not a sample.

**3. Wrap the engine as an autograd layer.** A `torch.autograd.Function` makes
the engine a differentiable op: `forward` returns the marginal, `backward`
returns `grad_out * ∂P/∂p` — the engine's gradient is the layer's Jacobian.

```python
class ReachProb(torch.autograd.Function):
    @staticmethod
    def forward(ctx, edge_probs, pattern):
        prob, grads = query(edge_probs.detach().tolist(), pattern)
        ctx.grads = torch.tensor(grads, dtype=edge_probs.dtype)
        return edge_probs.new_tensor(prob)

    @staticmethod
    def backward(ctx, grad_out):
        return grad_out * ctx.grads, None
```

**4. Train on a logical loss.** The loss is `-log P(reach(a,c))
- log(1 - P(reach(a,d)))` — pure logic, no edge labels. `loss.backward()`
routes the engine's exact gradient through the GNN's weights; Adam does the
rest.

## Running it

```sh
pip install ./crates/strata-py       # or: maturin develop  (from crates/strata-py)
pip install torch
python examples/python/train_gnn.py
```

Output (seeded, deterministic):

```
epoch   0  loss 1.9219
...
final  P(reach(a, c)) = 1.000   P(reach(a, d)) = 0.000
  edge(a, b) = 1.000
  edge(b, c) = 1.000
  edge(a, c) = 1.000
  edge(b, d) = 0.000
  edge(c, d) = 0.000
  edge(a, d) = 0.000
TRAINED
```

The model drove the positive query to 1 and the negative to 0 by believing
`a→b→c` (and the direct `a→c`) while disbelieving every edge into `d` — a
solution to the *logical* constraint, learned through the engine's gradient.
The same asserts run in CI ([`tests/test_train_gnn.py`](../../crates/strata-py/tests/test_train_gnn.py)),
so this recipe cannot rot silently.

## Where this is useful — and where it is not

Good fit: small, structured decision problems where the *rule* is known and
the *evidence* is learned — policy/compliance reachability, constraint-guided
extraction, explainable scoring with a hard logical target. The engine gives
an exact marginal and an exact gradient, and the provenance is inspectable.

Not a fit: large-N probabilistic inference. The exact marginal is a
possible-world computation with real cost; scale it with `Prov_k` lower bounds
and the proof budgets, and treat the exact path as an oracle / small-N engine,
not a throughput promise.
