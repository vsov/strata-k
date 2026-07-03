# Chapter 4 — Autopsies

Chapter 2 told you how the paradigm rose and fell; chapter 3 showed its fragments thriving under assumed names. This chapter answers the question those two leave hanging: if the idea is this good, why did the *languages* built on it stall? Not the strawman versions — the real ones, each of which got something profoundly right and each of which hit a wall.

I'll hold every system to the same three-part examination: **what it got right**, **what limited it**, and **what a successor should inherit**. And I'll grade the limits against three axes that didn't exist when these systems were designed, and that now decide everything:

- **Hardware.** Does the execution model parallelize? Modern performance is parallelism; a fundamentally sequential model has quietly lost the next thirty years.
- **Machine generation.** If an LLM writes programs in this language, do small mistakes stay small? Is meaning stable under reordering? Is a wrong program *rejected with a reason*, or does it silently misbehave?
- **Expressiveness under guarantee.** What can you say while keeping termination, determinism, and explainability — the properties that justified leaving your imperative language in the first place?

Each autopsy comes with a counterexample you can run. They live in `examples/book/ch04/`; the numbers below are from my machine, and the README there tells you how to reproduce them.

## Prolog: the search procedure ate the logic

**What it got right.** Almost everything this book has praised so far, Prolog said first, and said beautifully: program as knowledge, rules you can read aloud, one language for data and logic. Fifty years of imitators — including this one — stand on it. It also proved the idea could be *fast* for its era: Warren's abstract machine made a logic language compile to something competitive, on 1980s hardware, with 1980s memory.

**What limited it.** Prolog answers queries by a specific search procedure: try clauses top to bottom, try goals left to right, backtrack on failure. That procedure is the language's soul and its wound, because it makes *the order of your source code part of your program's meaning*. Here are the same three clauses from chapter 1, in Prolog, twice:

```prolog
% path_good.pl                      % path_bad.pl — same clauses, reordered
path(X,Y) :- edge(X,Y).             path(X,Z) :- path(X,Y), edge(Y,Z).
path(X,Z) :- edge(X,Y), path(Y,Z).  path(X,Y) :- edge(X,Y).
```

Logically these are identical — the same two sentences, in a different order on the page. The first answers `path(a,d)` instantly. The second recurses into `path` before it has consulted a single fact, forever:

```
$ swipl -g "..." path_good.pl      → yes
$ timeout 10 swipl -g "..." path_bad.pl   → killed after 10 s, no answer
```

Not a wrong answer — *no* answer, no error, no diagnostic. A logically correct program that hangs because two lines swapped places. Every Prolog programmer learns to write clauses in the magic order, which is another way of saying every Prolog programmer learns to hand-compile the control flow after all — the thing the paradigm promised to take away. And when the magic order isn't enough, Prolog offers the cut (`!`), an operator whose entire purpose is to reach into the search procedure and prune it. Cut is genuinely necessary in practice, and using it means your "declarative" program can no longer be read as sentences: it must be read as a *trace*.

Grade it against the axes. **Hardware:** depth-first backtracking is a fundamentally sequential walk of a proof tree; decades of parallel-Prolog research (and, as chapter 2 recounted, a Japanese national project) broke against it. **Machine generation:** meaning-depends-on-order is close to the worst property a generation target can have — an LLM that emits correct clauses in an unlucky arrangement produces a hang, the one failure mode that gives no error message to iterate on. **Expressiveness:** unbounded terms make Prolog Turing-complete, which sounds like a compliment but bills the programmer for it: termination is your problem, on every program, forever.

**What a successor inherits.** The dream itself — and the discipline to give it up per feature: Datalog is what remains of Prolog when you remove the things that made order matter. No term construction, no cut, no clause order, no goal order. Chapter 1's engine finds the fixpoint in *any* order because there is nothing in the language that can observe the order. Prolog's lesson is that the logic and the search must never share a syntax.

## Expert-system shells: rules without a referee

**What it got right.** The 1980s shells — OPS5, ART, CLIPS, and their enterprise descendants like Drools — made two moves the industry still hasn't fully absorbed. First: business rules deserve to live *outside* the application code, in a form domain experts can read — that's the sanctions smear from the prologue, diagnosed correctly, in 1985. Second, a technical gem: the Rete algorithm, which matches thousands of rules against changing facts *incrementally*, reusing partial matches instead of re-evaluating from scratch. Squint and you'll see the delta-driven evaluation from chapter 1's engine; the lineage of that trick runs straight through this book to its GPU chapters.

**What limited it.** I speak from the inside here: I rebuilt one of these engines, and the rebuilding is how I learned where the crack runs. It isn't in the matching — that part is brilliant. It's in what happens after the match. When several rules match at once, a shell consults a *conflict-resolution strategy* to pick which fires first — and CLIPS ships **seven** of them (depth, breadth, simplicity, complexity, LEX, MEA, random). Switch strategy, and the same rules on the same facts can fire in a different order; and since a rule's action can be *any side effect* — retract a fact, mutate a value, call out — a different firing order is, in general, a different final state. Read that back slowly: the rulebook alone does not determine the outcome; the rulebook plus a scheduling policy does. There is no fixpoint to trust, no model to check, no answer to the question "what do these rules, as such, entail?" A shell is not a declarative system; it is an event-driven imperative system whose statements *look like* rules.

The older cousins added a subtler warning about improvised mathematics. MYCIN — the 1970s medical expert system this whole product category descends from — attached "certainty factors" to its rules and combined them with ad-hoc formulas; researchers later showed the scheme is only coherent as probability under independence assumptions the rule base never satisfied. Numbers flowed through it and produced confident nonsense at scale. Chapter 7 will be this book's answer to that story: if you attach numbers to inference, the *arithmetic* of combining them is the entire game, and it cannot be improvised.

**Axes**, briefly: hardware — Rete parallelizes moderately, but the strategy-then-side-effect loop at the core is inherently serial; generation — an LLM-written rule can be individually plausible and collectively catastrophic, with no compiler able to object, since almost nothing is statically wrong; expressiveness — unbounded, and therefore unanalyzable.

**What a successor inherits.** Rules outside code: right. Incremental delta matching: right, and generalized. And one hard-won principle, stated as a negative — *no referee*. In Strata-K there is no agenda and no strategy; all rules "fire" mathematically simultaneously into a fixpoint, side effects don't exist, and two derivations of the same fact are one fact. What the shells needed was not a better conflict-resolution strategy but a semantics in which conflicts of that kind cannot arise.

## ASP: the right semantics behind two walls

**What it got right.** Answer Set Programming fixed the exact disease of the previous two patients. A stable model (chapter 8 will make friends with the definition) is a property of the *program* — which sets of conclusions are self-consistent — with no reference to any evaluation order, strategy, or procedure. Modern solvers like clingo are engineering triumphs, and the modeling experience is the paradigm at its most honest: state what a valid schedule *is*, not how to search for one. It has real industrial deployments — chapter 3 visited the seaport.

**What limited it.** Two walls, one at each end of the pipeline. The entry wall is **grounding**: before solving, every rule is instantiated with every applicable combination of constants. A rule with three variables over a domain of size *n* grounds to *n*³ instances. Here is a two-line program, measured:

```
node(1..N).
triple(X,Y,Z) :- node(X), node(Y), node(Z).

N = 10   →      1,010 ground rules
N = 50   →    125,050
N = 200  →  8,000,200   (7.5 s just to ground, before any solving)
```

Two hundred of anything is not big data — it's a small watchlist — and we're already holding eight million instantiations. Real encodings are cleverer, and real grounders are excellent; but the cliff is structural, every serious ASP practitioner spends real effort "grounding-aware modeling," and that phrase should sound familiar: it's the magic clause order again, one abstraction level up. The exit wall is **expressiveness of the answer**: a stable model is a set of atoms — yes/no, all the way down. The moment your problem says "usually", "how likely", or "at what cost" (weak constraints handle simple cost, nothing native handles likelihood), you are outside the language. And between the walls sits a solver whose core search — conflict-driven, learning, sequential — is precisely the kind of algorithm GPUs are worst at.

**Generation axis**, worth its own paragraph: ASP semantics is *global*. Adding one innocent-looking rule can extinguish every stable model of the program, and the solver's entire answer is `UNSATISFIABLE` — one word, no location, no witness. For a human expert this is a puzzle; for an LLM iterating on feedback it is a brick wall. Compare what a generation loop needs: local errors, with positions, with reasons.

**What a successor inherits.** The semantics itself — stable models are the correct meaning of unrestricted negation and choice, full stop. What changes is the *placement*: in Strata-K, ASP is an explicitly marked layer (`@asp`) you opt into for the subproblems that need choice, not the default cost of every program; the deductive core keeps its cheap, local, stratified world. The walls get attacked separately — grounding is a massively parallel enumeration (a GPU job, chapter 10), and the yes/no answer wall falls to the semiring machinery of chapter 7, which gives the *deductive* layer the "how likely / at what cost" vocabulary ASP never had.

## Constraint programming: the model that lies about its price

**What it got right.** CP begins from an insight this book fully endorses: constraints are knowledge. "These two meetings can't overlap" is a fact about the world, and propagating consequences of such facts — shrinking possibilities until only solutions remain — is inference, done by remarkably sophisticated engines. For scheduling, rostering, and configuration, CP solvers remain, to this day, some of the most effective tools humanity owns. Its cousin CHR distilled the idea to a beautiful minimum: computation as rewriting a store of constraints.

**What limited it.** The price of a CP model is invisible in its text. Here is the classic N-queens model in SWI-Prolog's constraint library, N = 26 — one model, two runs, the only difference being the `labeling` annotation that tunes search order:

```
labeling([],  Qs)   →  8.019 s
labeling([ff], Qs)  →  0.014 s        # ~570× — same constraints, same solutions
```

Five hundred seventy times, from a hint that has *no logical content whatsoever* — both runs solve the identical problem and return valid boards. And it cuts both ways: an equally innocent change to the *model* (one extra constraint, one symmetry left unbroken) can take a solve from milliseconds past your deadline, with nothing in the source marking the cliff edge. CP experts earn their living knowing where the cliffs are. That expertise is real, and it is exactly the hand-compilation tax again: the *what* is declarative; the *how* leaks back in through heuristics, and the heuristics decide whether you get an answer today.

**Axes.** Hardware: propagation is a fine-grained, dependency-riddled fixpoint plus sequential search — decades of parallel-CP work, persistently modest speedups. Generation: the worst case on this axis isn't wrong output, it's *plausible* output — an LLM produces a perfectly correct model that happens to be 570× too slow, and no type checker, linter, or test on small inputs will catch it. Expressiveness: within its combinatorial niche, superb; as a general substrate, narrow.

**What a successor inherits.** Two design rules, both already constitutional in Strata-K. First: performance must be *legible* — every language construct has a documented cost in the execution model, and anything with a hidden exponential inside must wear a syntactic marker (you will meet this in chapter 7: the expensive question is spelled `?prob`, and it looks expensive). Second: hints must be *sterile* — anything tunable may change the order of work, never the answer. The `labeling` flag actually honors that rule, and it's the right kind of knob; the failure is that the knob is load-bearing and unmarked. Declare the cliff in the language, or the language is lying.

## The matrix

Four autopsies, three axes, one table:

| | **Hardware** | **Machine generation** | **Expressiveness under guarantee** |
|---|---|---|---|
| **Prolog** | sequential backtracking; resists parallelism structurally | order changes meaning; failure mode is a silent hang | Turing-complete, termination unguaranteed everywhere |
| **Expert shells** | Rete partly parallel; strategy+side-effect loop serial | rules individually fine, collectively unpredictable; nothing statically checkable | side effects unbounded; no semantics to analyze |
| **ASP** | grounding parallelizes; core search does not | global semantics; one rule can yield bare `UNSATISFIABLE` | boolean answers only; no native cost/likelihood |
| **CP/CHR** | propagation fine-grained sequential; modest parallel gains | plausible-but-570×-slow output; cost invisible in source | strong in combinatorial niche; narrow substrate |

Read the table by columns and you can almost see the next language assembling itself. Column one demands an execution model that is parallel *by construction* — a fixpoint over sets, not a walk of a tree. Column two demands order-independence, locality of errors, and cost you can see in the source. Column three demands guarantees as the default and every escape hatch explicitly marked. None of the four patients died of a foolish idea; each died of one load-bearing coupling — logic to search, rules to scheduler, semantics to grounder, model to heuristic.

The next chapter turns this table upside down: it visits the systems being built right now, at the frontier, that each solve one column brilliantly — and shows that the puzzle piece missing is the language where the columns meet.

> **Run the autopsies.** All four counterexamples, with exact commands and my measured numbers: [`examples/book/ch04/`](../../examples/book/ch04/README.md).
