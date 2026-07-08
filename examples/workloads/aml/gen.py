#!/usr/bin/env python3
"""Deterministic data generator for the AML ownership-screen workload.

Writes companies.tsv / ownership.tsv / sanctions.tsv / flags.tsv next to
itself. Committed data == this script's output (CI does not re-run it; it
exists so the numbers are transparent and regenerable, not magic).
"""

import pathlib
import random

HERE = pathlib.Path(__file__).resolve().parent
rng = random.Random(20260708)

# --- 60 firms: 6 holding pyramids of 8, plus 12 independents ---------------
firms = []
pyramids = []
for g in range(6):
    group = [f"g{g}_hold", f"g{g}_mid1", f"g{g}_mid2"] + [f"g{g}_op{i}" for i in range(5)]
    pyramids.append(group)
    firms += group
independents = [f"indie{i}" for i in range(12)]
firms += independents

# --- ownership: each pyramid is a tree hold -> mid -> ops, plus cross-links -
edges = []
for group in pyramids:
    hold, mid1, mid2, *ops = group
    edges += [(hold, mid1), (hold, mid2)]
    for i, op in enumerate(ops):
        edges.append((mid1 if i % 2 == 0 else mid2, op))
# a few cross-pyramid stakes and indie holdings (no cycles: only forward group index)
for _ in range(14):
    a, b = rng.sample(range(6), 2)
    if a > b:
        a, b = b, a
    edges.append((pyramids[a][rng.randrange(3)], pyramids[b][3 + rng.randrange(5)]))
for i, indie in enumerate(independents[:6]):
    edges.append((pyramids[i][0], indie))
edges = sorted(set(edges))

# --- sanctions: two holdings and one independent ----------------------------
sanctions = ["g4_hold", "g5_hold", "indie11"]

# --- model flags: 10 soft rows, mostly on operating companies ---------------
flagged = ["g0_op1", "g1_op3", "g2_op0", "g2_op4", "g3_op2", "g4_op1", "g5_op0", "indie3", "indie7", "g0_mid2"]
flags = [(f, round(rng.uniform(0.35, 0.95), 2)) for f in flagged]

(HERE / "companies.tsv").write_text("".join(f"{f}\n" for f in sorted(firms)))
(HERE / "ownership.tsv").write_text("".join(f"{a}\t{b}\n" for a, b in edges))
(HERE / "sanctions.tsv").write_text("".join(f"{s}\n" for s in sanctions))
(HERE / "flags.tsv").write_text("".join(f"{f}\t{p}\n" for f, p in flags))
print(f"{len(firms)} firms, {len(edges)} ownership edges, {len(sanctions)} sanctioned, {len(flags)} soft flags")
