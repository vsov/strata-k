# The Strata/K language reference

This is a practical reference for the language as it runs today (Phase 0). It
describes the surface syntax and the semantics the reference interpreter
implements. For the formal grammar see [grammar.ebnf](grammar.ebnf); for the
JSON IR see [ir-encoding.md](ir-encoding.md) and
[`../schema/high-ir.schema.json`](../schema/high-ir.schema.json).

A Strata/K program has two interchangeable forms: the **surface syntax** shown
here, and a **High-IR JSON** document that is the canonical source of truth.
`strata ir --to json|surface` converts between them; `strata fmt` is the
surface-to-surface projection (`parse → print`).

## Lexical conventions

Strata/K follows the Prolog lexical convention, chosen so that a variable and a
constant are never confused:

- **Variables** start with an uppercase letter: `X`, `Node`, `Firm2`.
- **Constants** (symbols) start with a lowercase letter: `apex`, `node`, `a`.
- **Integers** are written as literals: `0`, `5`, `-3`.
- **Comments** start with `%` and run to end of line.

Whitespace and blank lines are insignificant to meaning; the canonical formatter
fixes their layout. Comments and blank lines are *trivia* — they are preserved
by `fmt` but excluded from semantic equality (two programs that differ only in
trivia are the same program).

## Declarations

### Domains

A `domain` introduces a type name used in predicate signatures:

```
domain node.
domain firm.
```

The built-in type `int` is always available for integer-valued columns.

### Predicate signatures (mandatory)

Every predicate must be declared before use. A signature gives each column a
type and the whole relation a **semiring**:

```
pred edge(node, node): Bool.
pred stake_count(firm, int): Bool.
pred route(node, node): Trop.
pred controls(firm, firm): Prov.
pred reach(node, node): Prov_k(2).
```

The four annotations: `Bool` (plain deduction), `Trop` (min-plus weights),
`Prov` (full provenance — see
[Provenance annotations](#provenance-annotations-prov--prov_k)), and
`Prov_k(k)` (top-k provenance, the recursion-safe approximation; bare `Prov_k`
means `Prov_k(3)`).

Mandatory signatures are a deliberate design choice: a mistyped predicate name
is a compile error (`E1001`), not a silently empty relation. Using a predicate
that was never declared, or with the wrong arity, is rejected.

## Facts

A fact is a ground atom (no variables). Depending on the head predicate's
semiring and any annotation, a fact may be plain, weighted, or probabilistic:

```
edge(a, b).            % a plain Bool fact
5 :: edge(a, b).       % a Trop (weighted) fact: weight 5
0.87 :: edge(a, b).    % a probabilistic fact: present with probability 0.87
```

- A plain fact has no annotation.
- `w :: atom` annotates a `Trop` fact with an integer weight `w` (`i64`).
- `p :: atom` annotates a soft fact with a probability `p ∈ [0, 1]` (see
  [Probabilistic queries](#probabilistic-queries)).

The `::` must fit the predicate's declared annotation, and the checker enforces
it (`E1009`): an integer weight belongs on `Trop` predicates only (`5 :: e(...)`
on a `Bool` predicate is the classic silent typo — a probability needs the
decimal point, `0.5 ::`); a probability belongs on `Bool`/`Prov`/`Prov_k`
predicates only and must lie in `[0, 1]`; and a `Trop` fact must carry a weight
(a bare tropical fact has no meaningful value to combine).

Facts must be ground; a fact containing a variable is rejected (`E1004`).

## Rules

A rule derives head atoms from a conjunctive body, Datalog-style:

```
path(X, Y) :- edge(X, Y).
path(X, Z) :- edge(X, Y), path(Y, Z).
```

The body is a comma-separated conjunction of literals. Rules may be recursive
(a predicate may appear in its own body, directly or through a cycle).

**Safety / range restriction.** Every variable in the head, and every variable
in a negated literal, must also appear in a positive body literal. A rule that
violates this is rejected (`E1003`) — it keeps every derivable relation finite.

**No singletons.** A variable that occurs exactly once in a rule is almost always
a typo, so it is an error (`E0010`), not a warning. Use a fresh name only where
you mean to.

## Negation

A body literal may be negated with `not`:

```
cleared(C) :- counterparty(C), not blocked(C).
```

In the deductive core, negation must be **stratified**: you cannot derive a
predicate from the negation of something that (transitively) depends on it. The
checker computes strata and rejects programs where negation or aggregation cycles
(`E1002`), guaranteeing a unique perfect model. Negated variables must be bound
by a positive literal (safety, above).

Unstratified negation *is* allowed inside an `@asp` module, where it is given
stable-model semantics instead (see [Answer sets](#answer-set-programming-asp)).

## Aggregates

An aggregate term in the head summarizes a variable over the rule's solutions:

```
stake_count(X, count<Y>) :- owns(X, Y).
outdeg(X, count<Y>)      :- edge(X, Y).
```

The aggregate functions are `min`, `max`, `sum`, and `count`. `prob_or`
remains **reserved, deliberately**: the specification assigns it no semantics,
and the language's constitution is *no arithmetic without a semantics* — a
noisy-or would also need a float-valued column, which `GroundVal` does not
have. It parses (the grammar is complete) and is refused at evaluation. If a
future revision gives it a semantics, the reservation is where it lands. The aggregated variable (`Y` above) is bound by the
body and consumed by the aggregate; the remaining head variables are the group
key. Aggregation is **non-recursive**: an aggregate reads only from lower strata,
like negation.

## Queries

`strata run` evaluates the program to its least fixpoint and prints the derived
relations. A plain `?q(...)` query is an **output filter**, not a computation:

```
?path(a, _).       % print only path tuples whose first argument is a
?edge(b, c).        % print only the ground tuple edge(b, c) (if derived)
```

A constant or integer position must match; a variable or `_` position matches
anything. With **no** `?` query the whole database prints; with one or more,
only the queried predicates print, and only their matching tuples. (The
probabilistic `?prob` / differentiable `?grad` forms are computations, not
filters — see [Probabilistic queries](#probabilistic-queries) and
[Gradient queries](#gradient-queries-grad).)

## Semirings

A predicate's semiring is the "arithmetic of inference" — the same rules compute
a different kind of answer depending on it:

| Semiring | Meaning | Example |
|---|---|---|
| `Bool` | ordinary Datalog: a tuple is derivable or not | reachability, membership |
| `Trop` | tropical (min-plus): least total cost | shortest paths, Viterbi |

`Trop` weights are `i64` with a distinct `+∞` (the additive identity). `⊕` is
`min`, `⊗` is integer addition; comparisons and results are bit-exact. Integer
overflow during `⊗` is a runtime fault (exit code `4`), never a silent wrap.

A rule may not mix incompatible semirings (e.g. feed a `Trop` body into a `Bool`
head); such conflicts are rejected (`E1007`). `Trop` output prints the weight:

```
$ strata run examples/tc.strata      # with Trop edges
reach(a, c) = 5
```

The `Prov` / `Prov_k` provenance semirings **run** — see
[Provenance annotations](#provenance-annotations-prov--prov_k). The full
annotation lattice is `Bool ⊑ Trop` and `Bool ⊑ Prov ⊑ Prov_k`, with `Trop` and
`Prov` incomparable (no homomorphism either way) and no edge back from soft to
certain — a `Prov` body cannot flow into a `Bool` head (`E1007`, the taint
discipline). A recursive `Prov` predicate reports `E1008` (the forbidden cell of
the semiring×recursion table) with the nearest allowed alternative, `Prov_k`.

## Probabilistic queries

A probabilistic fact `p :: a(...)` declares an independent Bernoulli event: the
atom is present with probability `p`. A `?prob` query asks for the marginal
probability of a derived tuple under **distribution semantics** (à la ProbLog):

```
pred edge(node, node): Bool.
pred path(node, node): Bool.
path(X, Y) :- edge(X, Y).
path(X, Z) :- edge(X, Y), path(Y, Z).

0.5 :: edge(a, c).
0.5 :: edge(a, b).
0.5 :: edge(b, c).

?prob path(a, c).
```

```
$ strata run examples/prob.strata
0.625 :: path(a, c)
```

The result is exact: `P(t) = Σ_worlds P(W)·[t derivable in W]`, computed by
enumerating all `2^n` possible worlds over the `n` probabilistic facts. This is
correct even when derivations share facts (the correlation case a naive semiring
convolution over-counts): here `min`-style over-counting would give `0.75`, but
the two routes to `c` share nothing and the true marginal is `0.625`. This is the
**режим B** reference — exact but exponential; knowledge compilation and top-k
are the "fast" methods a later phase must match against it. Exact enumeration is
refused past 20 probabilistic facts (it is #P-hard). The deductive part must be
`Bool`.

### Gradient queries (`?grad`)

A `?grad` query returns the marginal probability of each matching tuple **and**
the gradient of that marginal with respect to every probabilistic fact —
reverse-mode differentiation over the same possible-world chain:

```
?grad path(a, c).
```

```
$ strata run examples/grad.strata
0.625 :: path(a, c)
  ∂/∂[0.5 :: edge(a, c)] = 0.75
  ∂/∂[0.5 :: edge(a, b)] = 0.25
  ∂/∂[0.5 :: edge(b, c)] = 0.25
```

Each `∂/∂[...]` line is `∂P(tuple)/∂p_i` for that soft fact — exact (checked
against finite differences), including at boundary probabilities `0` and `1`.
This is the number a host training loop backpropagates into whatever produced
the probability.

## Provenance annotations (`Prov` / `Prov_k`)

A predicate annotated `Prov` carries **full provenance**: every derived fact
knows the minimal sets of soft facts it rests on — its *proofs*. Evaluation
captures the proofs during the fixpoint, compiles them into a
deterministic/decomposable circuit (Shannon expansion, exact even when proofs
share facts), and weighted model counting gives the marginal; the same circuit
runs backward for `?grad`. A plain `strata run` prints each fact's pedigree,
one `⇐` line per proof:

```
$ strata run examples/book/ch11-prov.strata
0.9 :: controls(acme, shell)
  ⇐ [0.9 :: owns(acme, shell)]
0.804 :: controls(acme, target)
  ⇐ [0.9 :: owns(acme, shell)] ∧ [0.8 :: owns(shell, target)]
  ⇐ [0.3 :: owns(acme, target)]
```

`?prob`/`?grad` queries against a `Prov`-annotated predicate go through the
same capture → compile → count pipeline instead of world enumeration. Where
enumeration is refused past 20 soft facts (`2^n` worlds), the circuit path
scales with the number of *proofs* — a 25-fact chain is answered exactly.

Exact provenance through recursion is impossible (a recursive soft fact has
infinitely many derivation trees; its exact provenance is a formal power
series), so a recursive `Prov` predicate is rejected (`E1008`) and the language
offers `Prov_k(k)` — keep the `k` best proofs per tuple (bare `Prov_k` means
`Prov_k(3)`). The reported marginal is then a **declared lower bound**, printed
as one — `0.5 :: reach(a, c)  (lower bound, top-1)` — monotone in `k` and equal
to exact once `k` covers every proof.

Stratified negation over soft provenance — derived or not — is exact: capture
takes the **complement** of the negated tuple's proof DNF (dual literals,
`x·x̄ = 0`, absorption), and the pedigree shows the absences as `¬[...]`
conjuncts. `@terms` programs work across the whole режим-B surface: one term
table is shared across worlds and through capture, so a constructed `f(a)` is
the same value everywhere. What capture still refuses, loudly: aggregating
over soft-supported tuples (correlated counting). The exact enumeration
oracle covers that world-by-world on `Bool` predicates, and the two pipelines
are differentially fuzzed against each other — including negation-over-soft
and `@terms` families.

Soft facts can also arrive and leave *incrementally*: the `strata-k` /
`strata-eval` `IncProv` maintainer updates the captured proofs without
recapturing (insert = resume the monotone fixpoint; delete = drop the leaf's
proofs — minimal leaf-free proofs are already present), for positive programs;
negation falls back to recapture, and every step is fuzz-checked against one.

Proof growth is budgeted, because real data can be exponential without asking
permission: exact capture refuses a tuple whose minimal-proof set passes
10 000 proofs, and circuit compilation refuses past 2²⁰ nodes — both with a
typed error naming the valve (`Prov_k`), never an OOM. `Prov_k` capture prunes
as it goes, so it stays inside the budget by construction.

The proof DNFs themselves are exportable — the library facade returns them as
signed-literal vectors, and the Python bridge (`Program.provenance()`,
`strata_k.wmc` / `wmc_grad`) hands them to external compilers in the same
shape. A real SDD package (PySDD) is compiled against them as a differential
oracle in CI: the external compiler's weighted count and the built-in Shannon
circuit's must agree, on fuzzed DNFs and on captured provenance alike.

## Neural predicates

A `neural` predicate declares that its ground atoms are the **soft outputs of a
model** — each fact carries the model's confidence, exactly like a probabilistic
fact, and flows into rules as ordinary soft evidence:

```
domain firm.
domain label.
neural flag(firm, label) from model "aml_gnn".

pred investigate(firm): Bool.
investigate(F) :- flag(F, structuring).

0.9 :: flag(acme, structuring).
?grad investigate(acme).
```

```
$ strata run
0.9 :: investigate(acme)
  ∂/∂[0.9 :: flag(acme, structuring)] = 1  (→ model "aml_gnn")
```

Facts on a neural predicate **must** be probabilistic — a certain fact on one is
a category error (`E1010`), because a model's outputs are inherently soft. A
`?grad` gradient on a neural fact is labelled with the model it backpropagates
into. The *model itself* is host-side: the CLI takes its outputs as data (inline
`p :: n(...)` facts or an `input` file with a trailing probability column),
and the `strata-k` library crate wires it **in-process** — a `Model` object's
forward pass supplies the soft facts at attach time, and gradients flow back
to it by position (see [crates/strata-k](../crates/strata-k)). The same
boundary crosses into Python: `pip install ./crates/strata-py`, then
`import strata_k`, attach a callable as the model, and the exact `?grad`
gradient arrives host-side — `examples/python/train_gnn.py` trains a PyTorch
network *through* the logic layer this way, the engine acting as one more
differentiable function. What stays open is production scale, not the wiring;
the interface — soft facts in, gradients out — is what the language defines.

## Structural terms (`@terms`)

A module that opens with the `@terms` pragma may use **constructor terms** —
`cons(H, T)`, `node(L, V, R)` — in any term position. This makes the language
Turing-complete, so termination is no longer guaranteed; the engine fences
divergence with a **depth bound**:

```
@terms.
domain elem.
pred nat(elem): Bool.
nat(zero).
nat(succ(N)) :- nat(N).
```

```
$ strata run examples/terms.strata
nat(zero)
nat(succ(zero))
nat(succ(succ(zero)))
…
% status: Sound (possibly incomplete) — 1 derivation(s) dropped at depth bound 64
```

- Ground compound terms are **hash-consed**: equal terms share one canonical id,
  and the engine compares scalars, never structures.
- Rule heads *construct* terms; rule bodies *decompose* them by unification —
  `root(V) :- tree(node(_L, V, _R))` matches a compound fact and binds `V`.
- A constructed term deeper than the bound is **dropped**, and the run is
  flagged `Sound (possibly incomplete)`: every printed fact is correct with
  respect to the unbounded program (terms are only dropped, never invented), but
  completeness is lost — the spec's `Sound[T]`, not `Complete[T]`.

## Answer-set programming (`@asp`)

A program that opens with the `@asp` pragma is an answer-set (stable model)
program. Here negation need not be stratified; the meaning is the set of
**stable models** (Gelfond–Lifschitz), each a self-consistent set of assumptions:

```
@asp.
pred node(node): Bool.
pred in(node): Bool.
pred out(node): Bool.

node(x).
node(y).

in(N)  :- node(N), not out(N).
out(N) :- node(N), not in(N).
```

```
$ strata run examples/asp.strata
Answer 1: {in(x), in(y), node(x), node(y)}
Answer 2: {in(x), node(x), node(y), out(y)}
Answer 3: {in(y), node(x), node(y), out(x)}
Answer 4: {node(x), node(y), out(x), out(y)}
```

The reference solver grounds over the Herbrand universe and enumerates stable
models via the reduct. A program with no stable model prints `UNSATISFIABLE`.
`@asp` skips the *stratifying* checker only — unstratified negation is the
point of stable-model semantics — while declarations, arity, and fact
groundness stay mandatory (`E1001`/`E1005`/`E1004`): the mistyped-predicate
promise is a global property of the language, not a per-mode courtesy. It
bypasses the semiring machinery because stable-model semantics is exactly what
handles the unstratified negation the deductive core forbids — and everything
that machinery would have owned is *refused by name* rather than silently
dropped (`E1011`): `::` fact annotations, compound (`@terms`) facts, queries,
`input` declarations, and `neural` predicates are not supported under `@asp`.
A grounding with an empty Herbrand universe simply has no instantiations (the
empty model, if stable, is the answer).

## Loading facts (EDB)

Facts may be written inline, or loaded from a file with `input` — **TSV, CSV,
or JSON**, dispatched by extension:

```
pred edge(node, node): Bool.
input edge from "edges.tsv".      % tab-separated (Soufflé-compatible)
input edge from "edges.csv".      % comma-separated; "quoted" fields, "" escapes
input edge from "edges.json".     % [["a","b"], ["b","c"]] — strings and integers
```

The path resolves relative to the source file; one row per fact, one column per
argument, symbols interned as constants. The trailing-column convention follows
the predicate:

- a `Trop` predicate takes one extra trailing **integer weight** column;
- a **`neural`** predicate takes one extra trailing **probability** column in
  `[0, 1]` — the model's outputs materialized to a file load as *soft* facts
  (the probabilistic EDB), never as certain ones: a row without the
  probability column is a load error, so the E1010 category error has no side
  door;
- every other predicate loads plain certain facts.

Value columns are **typed by the declaration**: an `int` column parses as an
integer in every format (`"5"` in a TSV, `5` in JSON, `5` inline are one
value — the value space never splits on load path), a domain column interns
non-empty text as a symbol. A row with the wrong number of columns, an empty
cell, a float or bare number where a symbol is declared, a non-integer in an
`int` column, or an unsupported extension is an error naming the file and
line. CSV quoting is strict: a quoted field must be whole (`"a"junk` is a load
error, not the constant `ajunk`), and a bare `"` inside an unquoted field is
refused — a corrupted export fails loudly instead of becoming data.

Loading is **atomic and once-only**. `load_inputs` validates every file and
row before committing anything: a failure partway through (a missing second
file, a bad row) leaves the program untouched, so a retry after fixing the
input is clean. A second load after success is a typed error — it would append
the same rows again and, for soft facts, silently shift every marginal. The
in-process twin `attach_models` keeps the same two promises (nothing committed
before full validation; a second attach refuses). For soft facts computed
*in-process* rather than materialized, use the `strata-k` library crate's
`Model` trait ([crates/strata-k](../crates/strata-k)).

## Diagnostics

Every diagnostic carries a stable code, a source location, and often a
machine-applicable fix. The front-end owns the `E0xxx` range, the checker the
`E1xxx` range:

| Code | Meaning |
|---|---|
| `E0001` | unexpected character (lexing) |
| `E0002` | parse: expected a different token/construct |
| `E0003` | parse: unexpected end of input |
| `E0010` | singleton variable (occurs exactly once) |
| `E0100` | construct is grammatical but not implemented (reserved — nothing in the current language triggers it) |
| `E1001` | predicate used but never declared |
| `E1002` | negation/aggregation through a cycle: not stratifiable |
| `E1003` | head/negated variable not range-restricted (safety) |
| `E1004` | fact contains a non-ground term |
| `E1005` | atom arity does not match its declaration |
| `E1006` | `Prov_k(0)` — the proof bound must be ≥ 1 |
| `E1007` | rule mixes incompatible semirings |
| `E1008` | forbidden cell of the semiring×recursion table (2.4) |
| `E1009` | fact `::` mismatch: `int ::` off `Trop`, probability on `Trop` or outside [0, 1], bare `Trop` fact |
| `E1010` | certain fact on a `neural` predicate (model outputs must be soft; a certain `input` row fails at load time with file:line) |
| `E1011` | construct with no meaning under `@asp`: `::` annotations, compound facts, queries, `input`, `neural` |
| `E1012` | predicate redeclared with a conflicting signature |

`strata check --error-format=json` emits the same diagnostics as machine-readable
JSON.

## Stability

Not every surface is equally settled. The **stable kernel** — the part whose
behavior and public API this project intends to keep — is:

- the `strata` CLI (`check` / `run` / `fmt` / `ir`) and its output format;
- the `strata_k` Python bridge (`compile`, `eval`, `prob_query`, `grad_query`,
  `provenance`, `attach_models`, `load_inputs`) and the `strata-k` Rust facade;
- the `Bool` and `Trop` semirings;
- probabilistic evaluation (режим B): `::` facts, `?prob` / `?grad`, and
  `Prov` / `Prov_k` provenance — exact marginals and gradients, with the
  documented budgets and `Prov_k` lower bounds;
- structural terms (`@terms`) with the depth-bounded, sound-possibly-incomplete
  status line;
- the `input` loaders (TSV / CSV / JSON) and plain `?q(...)` output filters;
- the worked examples: the two [workloads](../examples/workloads/README.md) and
  the [Python recipe](../examples/python/README.md), all pinned in CI.

Each of these is backed by a differential oracle and/or a pinned CI check.

**Experimental** — useful and tested, but the API or semantics may still move,
so depend on them with that in mind:

- **`@asp`** (answer-set / stable-model island): declarations are enforced,
  but the surface it exposes is narrower than the deductive core and still
  settling.
- **`IncProv`** (incremental provenance maintenance): the insert/delete
  contract is young (its index contract was just corrected); recapture is the
  conservative fallback.
- **the GPU engine** (`strata-gpu`, `--features cuda`): bit-exact against the
  reference oracle, but device-only, and its perf numbers are scoped to one
  box (see the crate README) — not a portable throughput promise.

## Not implemented in Phase 0

Nothing. Every construct in the shipped grammar now executes — `Prov`/`Prov_k`
were the last staged pieces. The future-syntax mechanism itself (parse into
valid IR, refuse by name with the stable `E0100`) remains the contract for any
construct a future revision stages.

The GPU execution engine and incremental (differential) evaluation live beside
this reference stack — see [../ARCHITECTURE.md](../ARCHITECTURE.md).
