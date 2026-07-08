#!/usr/bin/env python3
"""Deterministic data generator for the routing workload: an 8x8 grid with
random link costs plus a handful of express links. Writes links.tsv next to
itself; committed data == this script's output."""

import pathlib
import random

HERE = pathlib.Path(__file__).resolve().parent
rng = random.Random(20260708)
N = 8

def node(r, c):
    return f"n{r}_{c}"

rows = []
for r in range(N):
    for c in range(N):
        if c + 1 < N:  # east-west, both ways
            w = rng.randint(1, 9)
            rows += [(node(r, c), node(r, c + 1), w), (node(r, c + 1), node(r, c), w)]
        if r + 1 < N:  # north-south, both ways
            w = rng.randint(1, 9)
            rows += [(node(r, c), node(r + 1, c), w), (node(r + 1, c), node(r, c), w)]
# express links: cheap long hops out of the hub
for r, c, w in [(0, 7, 3), (7, 0, 4), (7, 7, 5), (4, 4, 2)]:
    rows.append((node(0, 0), node(r, c), w))

(HERE / "links.tsv").write_text("".join(f"{a}\t{b}\t{w}\n" for a, b, w in rows))
print(f"{N*N} nodes, {len(rows)} weighted links")
