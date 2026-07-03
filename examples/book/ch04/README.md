# Chapter 4 counterexamples — measured behavior

Environment: SWI-Prolog 10.0.2, clingo 5.8.0, Apple Silicon (arm64-darwin).
Numbers in the book text are from these runs; re-measure with the commands below.

- `path_good.pl` / `path_bad.pl` — the same three clauses, reordered.
  `swipl -g "consult('path_good.pl'), (path(a,d)->writeln(yes);writeln(no)), halt"` → `yes`.
  Same query on `path_bad.pl` → no answer; killed by `timeout 10` (exit 124).
- `queens.pl` — N=26 queens, identical constraints:
  `labeling([], Qs)` → 8.019 s CPU; `labeling([ff], Qs)` → 0.014 s (~570×).
- `ground200.lp` — 2-line program, 3-variable rule over domain N:
  ground instances: N=10 → 1,010 · N=50 → 125,050 · N=200 → 8,000,200 (`clingo --text | wc -l`), 7.48 s wall to ground.
