#!/usr/bin/env python3
"""Train a GNN *through* the logic layer.

A tiny message-passing GNN scores candidate edges; the program declares them
`neural edge(...) from model "gnn"` and asks for reachability. The training
signal is logical — "c must be reachable from a, d must not" — and the exact
gradient ∂P(reach)/∂p(edge) flows from the engine's `grad_query` back into the
GNN's weights through a custom autograd function. No proxy loss on individual
edges: the model learns *which edges to believe* so the derived relation comes
out right (keep a path a→c, cut every path a→d — a combinatorial trade, not a
per-edge label).

Run:  pip install torch && python examples/python/train_gnn.py
"""

import torch

import strata_k

torch.manual_seed(0)

NODES = ["a", "b", "c", "d"]
CANDIDATES = [("a", "b"), ("b", "c"), ("a", "c"), ("b", "d"), ("c", "d"), ("a", "d")]
POSITIVE = [("a", "c")]  # P(reach) → 1
NEGATIVE = [("a", "d")]  # P(reach) → 0

SRC = """
domain node.
neural edge(node, node) from model "gnn".
pred reach(node, node): Bool.
reach(X, Y) :- edge(X, Y).
reach(X, Z) :- edge(X, Y), reach(Y, Z).
"""


def query(edge_probs, pattern):
    """P(reach(pattern)) and its gradient per candidate edge, from the engine."""
    probs = [float(p) for p in edge_probs]
    program = strata_k.compile(SRC)
    program.attach_models(
        {"gnn": lambda: [("edge", e, probs[i]) for i, e in enumerate(CANDIDATES)]}
    )
    # gradient positions follow prob_facts() — which is exactly our fact order
    assert [f[1] for f in program.prob_facts()] == CANDIDATES
    ((_, prob, grads),) = program.grad_query("reach", list(pattern))
    return prob, grads


class ReachProb(torch.autograd.Function):
    """The engine as a differentiable layer: exact marginal, exact gradient."""

    @staticmethod
    def forward(ctx, edge_probs, pattern):
        prob, grads = query(edge_probs.detach().tolist(), pattern)
        ctx.grads = torch.tensor(grads, dtype=edge_probs.dtype)
        return edge_probs.new_tensor(prob)

    @staticmethod
    def backward(ctx, grad_out):
        return grad_out * ctx.grads, None


class TinyGNN(torch.nn.Module):
    """Two rounds of mean-aggregation message passing, then an edge scorer."""

    def __init__(self, nodes, edges, dim=8):
        super().__init__()
        self.index = {n: i for i, n in enumerate(nodes)}
        self.edges = edges
        self.inbound = {
            self.index[v]: [self.index[u] for (u, w) in edges if w == v] for v in nodes
        }
        self.emb = torch.nn.Embedding(len(nodes), dim)
        self.msg = torch.nn.Linear(dim, dim)
        self.upd = torch.nn.Linear(2 * dim, dim)
        self.score = torch.nn.Sequential(
            torch.nn.Linear(2 * dim, dim), torch.nn.ReLU(), torch.nn.Linear(dim, 1)
        )

    def forward(self):
        h = self.emb.weight
        for _ in range(2):
            m = self.msg(h)
            agg = torch.stack(
                [
                    m[ins].mean(dim=0) if ins else torch.zeros_like(m[0])
                    for i, ins in sorted(self.inbound.items())
                ]
            )
            h = torch.relu(self.upd(torch.cat([h, agg], dim=1)))
        logits = torch.stack(
            [
                self.score(torch.cat([h[self.index[u]], h[self.index[v]]]))
                for (u, v) in self.edges
            ]
        )
        return torch.sigmoid(logits.squeeze(-1))


def main():
    model = TinyGNN(NODES, CANDIDATES)
    opt = torch.optim.Adam(model.parameters(), lr=0.05)
    first_loss = None
    for epoch in range(80):
        edge_probs = model()
        loss = edge_probs.new_tensor(0.0)
        for q in POSITIVE:
            loss = loss - torch.log(ReachProb.apply(edge_probs, q).clamp_min(1e-6))
        for q in NEGATIVE:
            loss = loss - torch.log((1 - ReachProb.apply(edge_probs, q)).clamp_min(1e-6))
        opt.zero_grad()
        loss.backward()
        opt.step()
        if first_loss is None:
            first_loss = loss.item()
        if epoch % 10 == 0:
            print(f"epoch {epoch:3d}  loss {loss.item():.4f}")

    edge_probs = model().detach()
    p_pos, _ = query(edge_probs.tolist(), POSITIVE[0])
    p_neg, _ = query(edge_probs.tolist(), NEGATIVE[0])
    print(f"final  P(reach(a, c)) = {p_pos:.3f}   P(reach(a, d)) = {p_neg:.3f}")
    for (u, v), p in zip(CANDIDATES, edge_probs.tolist()):
        print(f"  edge({u}, {v}) = {p:.3f}")

    assert loss.item() < first_loss, "training did not reduce the loss"
    assert p_pos > 0.8, "positive query did not learn"
    assert p_neg < 0.2, "negative query did not learn"
    print("TRAINED")


if __name__ == "__main__":
    main()
