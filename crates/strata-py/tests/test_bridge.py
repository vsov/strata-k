# The Python bridge, end to end: every exported function against known-good
# answers (the same fixtures the Rust facade tests pin down).
import math

import pytest
import strata_k

DIAMOND = """
domain node.
pred edge(node, node): Bool.
pred path(node, node): Prov_k(4).
path(X, Y) :- edge(X, Y).
path(X, Z) :- edge(X, Y), path(Y, Z).
0.9 :: edge(a, b).
0.8 :: edge(b, c).
0.5 :: edge(a, c).
"""


def test_eval_bool_and_trop_rows():
    p = strata_k.compile(
        "pred e(int, int): Bool.\n"
        "pred t(int, int): Trop.\n"
        "e(1, 2).\n"
        "3 :: t(1, 2).\n"
    )
    db = p.eval()
    assert db["e"] == [(1, 2)]
    assert db["t"] == [(1, 2, 3)]  # Trop rows carry the trailing weight


def test_compile_diagnostics_raise_value_error():
    with pytest.raises(ValueError, match="E1008"):
        strata_k.compile(
            "pred e(int, int): Bool.\n"
            "pred p(int, int): Prov.\n"
            "p(X, Y) :- e(X, Y).\n"
            "p(X, Z) :- e(X, Y), p(Y, Z).\n"
        )


def test_prob_and_grad_query():
    p = strata_k.compile(DIAMOND)
    ((row, prob),) = p.prob_query("path", ["a", "c"])
    assert row == ("a", "c")
    # P(ab·bc ∨ ac) = 1 - (1 - 0.72)(1 - 0.5)
    assert prob == pytest.approx(0.86)

    ((row, prob, grads),) = p.grad_query("path", ["a", "c"])
    facts = p.prob_facts()
    assert len(grads) == len(facts) == 3
    by_edge = {f[1]: g for f, g in zip(facts, grads)}
    # ∂P/∂p(ac) = 1 - P(ab·bc) = 0.28
    assert by_edge[("a", "c")] == pytest.approx(0.28)


def test_pattern_wildcards_and_arity_errors():
    p = strata_k.compile(DIAMOND)
    rows = p.prob_query("path")  # omitted pattern = all rows
    assert len(rows) == 3
    rows = p.prob_query("path", [None, "c"])
    assert {r for r, _ in rows} == {("a", "c"), ("b", "c")}
    with pytest.raises(ValueError, match="arity"):
        p.prob_query("path", ["a"])
    with pytest.raises(ValueError, match="unknown predicate"):
        p.prob_query("nope")


def test_provenance_literals_align_with_prob_facts():
    p = strata_k.compile(DIAMOND)
    prov = dict(p.provenance()["path"])
    facts = p.prob_facts()
    # literal ±(i+1) indexes prob_facts(): resolve each proof back to edges
    idx = {i + 1: facts[i][1] for i in range(len(facts))}
    proofs = {frozenset(idx[l] for l in proof) for proof in prov[("a", "c")]}
    assert proofs == {
        frozenset({("a", "b"), ("b", "c")}),
        frozenset({("a", "c")}),
    }


def test_wmc_matches_prob_query():
    p = strata_k.compile(DIAMOND)
    proofs = dict(p.provenance()["path"])[("a", "c")]
    probs = [f[2] for f in p.prob_facts()]
    assert strata_k.wmc(proofs, probs) == pytest.approx(0.86)
    total, grads = strata_k.wmc_grad(proofs, probs)
    assert total == pytest.approx(0.86)
    assert len(grads) == 3


def test_wmc_input_validation():
    with pytest.raises(ValueError, match="out of range"):
        strata_k.wmc([[3]], [0.5, 0.5, 0.5][:2])
    with pytest.raises(ValueError, match="outside"):
        strata_k.wmc([[1]], [1.5])


def test_attach_models_in_process():
    p = strata_k.compile(
        "domain firm.\n"
        'neural flag(firm) from model "risk".\n'
        "pred investigate(firm): Bool.\n"
        "investigate(X) :- flag(X).\n"
    )
    assert p.prob_facts() == []  # nothing pasted in the source
    p.attach_models({"risk": lambda: [("flag", ("acme",), 0.9)]})
    assert p.prob_facts() == [("flag", ("acme",), 0.9)]
    ((row, prob, grads),) = p.grad_query("investigate", ["acme"])
    assert prob == pytest.approx(0.9)
    assert grads[0] == pytest.approx(1.0)


def test_attach_models_typed_errors():
    src = (
        "domain firm.\n"
        'neural flag(firm) from model "risk".\n'
        "pred investigate(firm): Bool.\n"
        "investigate(X) :- flag(X).\n"
    )
    with pytest.raises(RuntimeError, match="none was attached"):
        strata_k.compile(src).attach_models({})
    with pytest.raises(RuntimeError, match="not declared"):
        strata_k.compile(src).attach_models(
            {"risk": lambda: [("investigate", ("acme",), 0.5)]}
        )
    with pytest.raises(RuntimeError, match="arity"):
        strata_k.compile(src).attach_models(
            {"risk": lambda: [("flag", ("acme", "extra"), 0.5)]}
        )
    with pytest.raises(RuntimeError, match=r"outside \[0, 1\]"):
        strata_k.compile(src).attach_models({"risk": lambda: [("flag", ("acme",), 1.5)]})
    # a Python-side failure surfaces as the Python exception, not an engine error
    def boom():
        raise KeyError("model exploded")

    with pytest.raises(KeyError, match="model exploded"):
        strata_k.compile(src).attach_models({"risk": boom})


def test_undeclared_models_are_never_run():
    # a registry with extra models: the undeclared callable must not run at
    # all — not merely have its facts discarded downstream
    p = strata_k.compile(
        "domain firm.\n"
        'neural flag(firm) from model "risk".\n'
        "pred investigate(firm): Bool.\n"
        "investigate(X) :- flag(X).\n"
    )

    def boom():
        raise RuntimeError("undeclared model executed")

    p.attach_models({"risk": lambda: [("flag", ("acme",), 0.9)], "extra": boom})
    assert p.prob_facts() == [("flag", ("acme",), 0.9)]


def test_second_attach_raises_instead_of_duplicating():
    p = strata_k.compile(
        "domain firm.\n"
        'neural flag(firm) from model "risk".\n'
        "pred investigate(firm): Bool.\n"
        "investigate(X) :- flag(X).\n"
    )
    model = {"risk": lambda: [("flag", ("acme",), 0.9)]}
    p.attach_models(model)
    with pytest.raises(RuntimeError, match="already called"):
        p.attach_models(model)
    # the failed second call left nothing behind: still one fact, P = 0.9
    assert p.prob_facts() == [("flag", ("acme",), 0.9)]
    ((_, prob),) = p.prob_query("investigate")
    assert prob == pytest.approx(0.9)


def test_terms_render_structurally():
    p = strata_k.compile(
        "@terms.\n"
        "domain item.\n"
        "pred base(item): Bool.\n"
        "pred wrap(item): Bool.\n"
        "wrap(f(X)) :- base(X).\n"
        "base(a).\n"
    )
    assert p.eval()["wrap"] == [("f(a)",)]


def test_asp_models():
    src = (
        "@asp.\n"
        "pred node(node): Bool.\n"
        "pred in(node): Bool.\n"
        "pred out(node): Bool.\n"
        "node(x).\n"
        "in(N) :- node(N), not out(N).\n"
        "out(N) :- node(N), not in(N).\n"
    )
    models = strata_k.asp_models(src)
    as_sets = {frozenset(m) for m in models}
    assert as_sets == {
        frozenset({("node", ("x",)), ("in", ("x",))}),
        frozenset({("node", ("x",)), ("out", ("x",))}),
    }


def test_queries_introspection():
    p = strata_k.compile(
        "pred e(int, int): Bool.\n"
        "e(1, 2).\n"
        "?e(1, _).\n"
    )
    assert p.queries() == [("plain", "e", [1, None])]


def test_load_inputs(tmp_path):
    (tmp_path / "edge.tsv").write_text("1\t2\n2\t3\n")
    p = strata_k.compile(
        "pred edge(int, int): Bool.\n"
        "pred tc(int, int): Bool.\n"
        "tc(X, Y) :- edge(X, Y).\n"
        "tc(X, Z) :- edge(X, Y), tc(Y, Z).\n"
        'input edge from "edge.tsv".\n'
    )
    p.load_inputs(str(tmp_path))
    assert p.eval()["tc"] == [(1, 2), (1, 3), (2, 3)]


def test_missing_input_dir_is_an_error(tmp_path):
    p = strata_k.compile(
        "pred edge(int, int): Bool.\n" 'input edge from "edge.tsv".\n'
    )
    with pytest.raises(ValueError):
        p.load_inputs(str(tmp_path / "missing"))


def test_second_load_inputs_raises_and_leaves_state_unchanged(tmp_path):
    (tmp_path / "flags.tsv").write_text("acme\t0.9\n")
    p = strata_k.compile(
        "domain firm.\n"
        'neural flag(firm) from model "m".\n'
        "pred investigate(firm): Bool.\n"
        "investigate(X) :- flag(X).\n"
        'input flag from "flags.tsv".\n'
    )
    p.load_inputs(str(tmp_path))
    assert p.prob_facts() == [("flag", ("acme",), 0.9)]
    # A second load would double the soft fact and shift the marginal 0.9 -> 0.99.
    with pytest.raises(ValueError, match="already called"):
        p.load_inputs(str(tmp_path))
    assert p.prob_facts() == [("flag", ("acme",), 0.9)]
    ((_, prob),) = p.prob_query("investigate")
    assert prob == pytest.approx(0.9)
