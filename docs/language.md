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
```

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
- `p :: atom` annotates a Bool fact with a probability `p ∈ [0, 1]` (see
  [Probabilistic queries](#probabilistic-queries)).

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

The aggregate functions are `min`, `max`, `sum`, and `count` (`prob_or` is
reserved for a later phase). The aggregated variable (`Y` above) is bound by the
body and consumed by the aggregate; the remaining head variables are the group
key. Aggregation is **non-recursive**: an aggregate reads only from lower strata,
like negation.

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

The `Prov` / `Prov_k` provenance semirings are designed but **not implemented in
Phase 0**; a predicate annotated with them parses and then reports `E1006`, and a
recursive `Prov` additionally reports `E1008` (the forbidden cell of the
semiring×recursion table) with the nearest allowed alternative.

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

`?grad` (gradient) queries parse but are **not implemented in Phase 0**.

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
`@asp` bypasses the stratifying checker (parse-level well-formedness suffices),
because stable-model semantics is exactly what handles the unstratified negation
the deductive core forbids.

## Loading facts (EDB)

Facts may be written inline, or loaded from a tab-separated file with `input`:

```
pred edge(node, node): Bool.
input edge from "edges.tsv".
```

The path resolves relative to the source file. The TSV format is
Soufflé-compatible: one row per fact, one tab-separated column per argument, each
interned as a symbol constant. A `Trop` predicate has one extra trailing integer
column for the weight. A row with the wrong number of columns is an error.

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
| `E0100` | construct is grammatical but not implemented in Phase 0 |
| `E1001` | predicate used but never declared |
| `E1002` | negation/aggregation through a cycle: not stratifiable |
| `E1003` | head/negated variable not range-restricted (safety) |
| `E1004` | fact contains a non-ground term |
| `E1005` | atom arity does not match its declaration |
| `E1006` | annotation not executable in Phase 0 (`Prov`/`Prov_k`) |
| `E1007` | rule mixes incompatible semirings |
| `E1008` | forbidden cell of the semiring×recursion table (2.4) |

`strata check --error-format=json` emits the same diagnostics as machine-readable
JSON.

## Not implemented in Phase 0

These constructs **parse into valid IR** and then return a stable
*"not implemented in Phase 0"* diagnostic — the surface is complete, execution is
staged:

- `neural` predicates
- `@terms` (structural terms: lists, trees under constructors)
- `Prov` / `Prov_k` provenance annotations
- `?grad` (gradient) queries

The GPU execution engine, knowledge-compilation optimization of режим B, and
incremental (differential) evaluation are later phases — see
[../ARCHITECTURE.md](../ARCHITECTURE.md).
