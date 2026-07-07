# The external-compiler differential: our Shannon-compiled circuit vs a real
# SDD package (PySDD, the UCLA SDD library) vs brute-force enumeration —
# three independent counters, one answer.
#
# PySDD missing → skipped locally as a courtesy; STRATA_REQUIRE_ORACLES=1
# (CI) turns absence into a hard failure, same contract as souffle/clingo.
import itertools
import os
import random

import pytest
import strata_k

REQUIRE = os.environ.get("STRATA_REQUIRE_ORACLES") == "1"
try:
    from pysdd.sdd import SddManager

    HAVE_PYSDD = True
except ImportError:
    HAVE_PYSDD = False

if REQUIRE:
    assert HAVE_PYSDD, "STRATA_REQUIRE_ORACLES=1 but the pysdd package is missing"

pytestmark = pytest.mark.skipif(not HAVE_PYSDD, reason="pysdd not installed")


def sdd_wmc(proofs, probs):
    """Compile the proof DNF with PySDD and weighted-model-count it."""
    mgr = SddManager(var_count=len(probs))
    node = mgr.false()
    for proof in proofs:
        term = mgr.true()
        for lit in proof:
            term = term & mgr.literal(lit)
        node = node | term
    w = node.wmc(log_mode=False)
    for i, p in enumerate(probs, start=1):
        w.set_literal_weight(mgr.literal(i), p)
        w.set_literal_weight(mgr.literal(-i), 1.0 - p)
    return w.propagate()


def brute_wmc(proofs, probs):
    """The slow truth: sum world weights over all 2^n assignments."""
    total = 0.0
    n = len(probs)
    for world in itertools.product([False, True], repeat=n):
        if any(all(world[abs(l) - 1] == (l > 0) for l in proof) for proof in proofs):
            weight = 1.0
            for i in range(n):
                weight *= probs[i] if world[i] else 1.0 - probs[i]
            total += weight
    return total


def test_known_dnf():
    proofs = [[1, 2], [-1, 3]]
    probs = [0.9, 0.8, 0.5]
    expected = 0.9 * 0.8 + 0.1 * 0.5
    assert strata_k.wmc(proofs, probs) == pytest.approx(expected)
    assert sdd_wmc(proofs, probs) == pytest.approx(expected)


def test_unused_leaves_normalize_to_one():
    # leaf 3 appears in no proof: both counters must marginalize it away
    proofs = [[1], [2]]
    probs = [0.3, 0.4, 0.7]
    expected = 1 - 0.7 * 0.6
    assert strata_k.wmc(proofs, probs) == pytest.approx(expected)
    assert sdd_wmc(proofs, probs) == pytest.approx(expected)


def test_fuzz_random_dnfs_triple_differential():
    rng = random.Random(20260707)
    for _ in range(200):
        n = rng.randint(1, 7)
        probs = [round(rng.uniform(0.05, 0.95), 3) for _ in range(n)]
        num_proofs = rng.randint(0, 6)
        proofs = []
        for _ in range(num_proofs):
            size = rng.randint(1, n)
            lits = rng.sample(range(1, n + 1), size)
            proofs.append([l if rng.random() < 0.7 else -l for l in lits])
        ours = strata_k.wmc(proofs, probs)
        sdd = sdd_wmc(proofs, probs)
        brute = brute_wmc(proofs, probs)
        assert ours == pytest.approx(brute, abs=1e-9), (proofs, probs)
        assert sdd == pytest.approx(brute, abs=1e-9), (proofs, probs)


def test_captured_provenance_through_pysdd():
    # end to end: capture proofs from a program, count them three ways,
    # and agree with the possible-world oracle (prob_query)
    p = strata_k.compile(
        "domain node.\n"
        "pred edge(node, node): Bool.\n"
        "pred path(node, node): Prov_k(8).\n"
        "path(X, Y) :- edge(X, Y).\n"
        "path(X, Z) :- edge(X, Y), path(Y, Z).\n"
        "0.9 :: edge(a, b).\n"
        "0.8 :: edge(b, c).\n"
        "0.5 :: edge(a, c).\n"
        "0.7 :: edge(c, d).\n"
        "0.6 :: edge(b, d).\n"
    )
    probs = [f[2] for f in p.prob_facts()]
    prov = dict(p.provenance()["path"])
    marginals = dict(p.prob_query("path"))
    assert set(prov) == set(marginals)
    for row, proofs in prov.items():
        ours = strata_k.wmc(proofs, probs)
        assert ours == pytest.approx(marginals[row], abs=1e-9), row
        assert sdd_wmc(proofs, probs) == pytest.approx(marginals[row], abs=1e-9), row
