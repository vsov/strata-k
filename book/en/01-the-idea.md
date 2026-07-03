# Chapter 1 — The Idea: What, Not How

## A loop you have already written

Let's start with work you have done before. A company can own another company, which can own another. Given the direct ownership links, compute *ultimate control*: who sits above whom, at any depth. This is the skeleton of the sanctions rule from the prologue, and it is also — strip away the domain — plain transitive closure, a computation you have implemented more times than you can count: build dependencies, org charts, network reachability, `#include` graphs, GC root scanning. It is always the same loop.

In Java it comes out something like this:

```java
Set<Edge> controls = new HashSet<>();
Deque<Edge> work = new ArrayDeque<>(owns);

while (!work.isEmpty()) {
    Edge e = work.pop();
    if (!controls.add(e)) continue;          // already known
    for (Company next : ownsIndex.get(e.to()))
        work.push(new Edge(e.from(), next)); // extend to the right
}
```

Ten lines, and you could write them half asleep. Now look at what those ten lines quietly demand of you.

The `continue` on the duplicate check is load-bearing: remove it and cyclic ownership — which absolutely occurs in real corporate structures, usually on purpose — loops forever. The choice to extend edges only "to the right" is load-bearing too. Is right-extension alone actually sufficient, or do you also need to join derived edges with each other? For this particular problem, right-extension suffices — but that's a small theorem, and you proved it in your head, and you wrote the proof down nowhere. The next maintainer gets the loop, not the theorem. And there's a third demand, the expensive one: this code computes the answer *once*. When tomorrow's onboarding adds three ownership links, recomputing from scratch is wasteful, so someone will eventually write the incremental version — and the incremental version is a genuinely different and harder program, with its own theorem, also unwritten.

None of this is the problem statement. "Who ultimately controls whom" says nothing about worklists, visited sets, extension direction, or increments. All of that is *how*. You supplied it, by hand, because your language cannot say *what*.

## The same program, as what

Here is the whole thing in Strata-K:

```
domain company.
pred owns(company, company): Bool.
pred controls(company, company): Bool.

controls(X, Y) :- owns(X, Y).
controls(X, Z) :- owns(X, Y), controls(Y, Z).

owns(apex, brightwater).
owns(brightwater, cobalt).
owns(cobalt, dunlin).
```

Read the two middle lines aloud; the syntax is designed to be read aloud. The symbol `:-` is "if", the comma is "and", capitalized names are variables, lowercase names are concrete things:

- *X controls Y if X owns Y.*
- *X controls Z if X owns some Y, and that Y controls Z.*

That's the entire logic — two sentences a compliance officer could check against the regulation. The lines below them are **facts**: Apex owns Brightwater, Brightwater owns Cobalt, Cobalt owns Dunlin. The lines above are **rules**: sentences with variables, true for *any* companies you substitute in. Facts are your data. Rules are your knowledge. There is no third thing.

Run it:

```
$ strata run examples/book/ch01-ownership.strata
controls(apex, brightwater)
controls(apex, cobalt)
controls(apex, dunlin)
controls(brightwater, cobalt)
controls(brightwater, dunlin)
controls(cobalt, dunlin)
owns(apex, brightwater)
owns(brightwater, cobalt)
owns(cobalt, dunlin)
```

Every ownership fact we stated, plus every control relationship that *follows* from what we stated. Apex controls Dunlin through a chain three deep, and nobody wrote a loop.

## What the engine actually does

No magic is about to be revealed, and that's the point — you already know the algorithm, because you've written it by hand.

The engine takes the rules and applies them to the facts it has. *X controls Y if X owns Y* — so all three `owns` facts produce three `controls` facts. Then it applies the rules again, to everything it now knows: the second rule can now combine `owns(apex, brightwater)` with the freshly derived `controls(brightwater, cobalt)` to conclude `controls(apex, cobalt)`. Then it applies the rules *again*. At some point a full pass derives nothing it hasn't already got — and it stops, because if a pass adds nothing, every future pass adds nothing too.

That's a do-while loop. Yours, in fact — the worklist code above is this exact scheme, hand-compiled for one specific pair of rules. The stopping point has a name, and it's the first vocabulary word of this book worth memorizing: the **fixpoint** — the state where applying every rule yields nothing new. "Run the program" means "find the fixpoint." Your `while (!work.isEmpty())` was a fixpoint computation all along; you just weren't given the word, or the machine that owns the loop for you.

Two guarantees come with handing the loop over, and both would have been your bugs to make.

**It always terminates.** Look at the right-hand side of the rules: variables, and nothing else. There is no `new`. A rule can only combine constants that already exist — it can conclude `controls(apex, dunlin)`, but it cannot mint a company that was never mentioned. A finite set of companies has a finite set of possible `controls` facts, the engine only ever adds facts, so the loop provably stops. Cycles in the data? Apex owns Brightwater owns Apex? The engine derives that each controls the other — and stops. The infinite loop you guarded against with that load-bearing `continue` is not merely handled; it is *impossible to express*.

**Order doesn't matter.** Swap the two rules. Shuffle the facts. Split them across ten files and load them in any order. The fixpoint is the same set of facts, every time — not as a matter of engine politeness, but of arithmetic: the answer is defined as *everything derivable from what you stated*, and derivability doesn't care what line something was written on. Hold this property; it looks like a convenience and it is actually a foundation. Order-independence is what died in the older logic languages (chapter 4 does the autopsy), and it is what makes machine-generated rules checkable at all (chapter 9 builds on it).

## What, not how

Notice everything the Strata-K program does *not* say. It does not say to walk the graph left to right. It does not maintain a visited set. It does not choose breadth-first against depth-first. It does not schedule. All of that — the entire content of your ten Java lines — has become the engine's problem, and the engine is free to solve it well: to pick join orders, to index, to process only the *delta* (the facts new since the last pass — the trick that turns naive rule-running into something competitive, and, much later in this book, into something massively parallel).

You have seen this trade before, and you liked it. SQL made exactly this move for lookups: you write `WHERE customer.id = order.customer_id`, and the planner — not you — decides hash join or index scan. Nobody mourns the hand-rolled B-tree traversals. Logic programming is the same bargain extended from *lookup* to *inference*: not just "find the rows where", but "find everything that follows from". And your database has, in fact, already smuggled a fragment of this paradigm into production behind your back — `WITH RECURSIVE`, the SQL feature everyone writes once, gets wrong twice, and wraps in a comment begging nobody to touch it, is precisely this chapter's two rules wearing a straitjacket. Chapter 3 returns to that.

There is a deeper consequence, easy to miss. The Java version is a *function*: it computes `controls`, and if next month you need "which companies does nobody control" or "how many entities sit under Apex", you write more functions, each with its own loop, its own bugs, its own theorem. The Strata-K version is *knowledge*: the two rules define what control means, and any number of questions can be asked against that same definition. New requirement, new rule or new query — not a new algorithm. The program stops being a pile of procedures that happen to agree and becomes a single model of the domain that answers questions. That is what "program = knowledge" means, and it's why the sanctions sentence from the prologue can be *the program* instead of the program's lost inspiration.

One more thread, planted here, paid off in chapter 7: because the engine derives every fact mechanically from stated facts and rules, it can remember *how* — every derived fact has a pedigree. When compliance asks "why was this trade blocked?", the answer is not "attach a debugger"; the answer is the derivation chain itself: *blocked because controls(apex, dunlin), because owns(apex, brightwater) and owns(brightwater, cobalt) and owns(cobalt, dunlin), and dunlin is listed.* An answer that shows its work. This book's title lives in that sentence.

## What this is not

A short honesty section, because "the engine figures it out" has burned you before.

This is not AI, in the current sense of the word. Nothing here is learned, statistical, or approximate. The fixpoint is deterministic mathematics — same facts, same rules, same answer, bit for bit, forever. (The connection to LLMs, this book's real motivation, is division of labor, not fusion: the model *proposes* rules; this machinery *guarantees* what follows. That story is chapter 9's.)

This is not a general-purpose language, either. You cannot write a web server in it, and that is a feature bought deliberately: the no-`new` restriction that guarantees termination costs expressive power, and the language spends that coin on purpose. Chapter 6 begins the tour of what the restriction buys and where the boundary actually sits.

And it is not a silver bullet. Most of your codebase is legitimately procedural and should stay imperative. The claim is narrower and sharper: the *rules* buried in that codebase — compliance, limits, routing, eligibility — are logic programs today, written in a language that can't see them. They deserve a language that can.

## The idea, and the question it raises

One chapter, four words of vocabulary, and the whole paradigm is on the table:

> A program is a set of **facts** (data) and **rules** (knowledge). Running it means deriving everything that follows — computing the **fixpoint**. A **query** asks about the result. You state *what*; the engine owns *how*.

Everything else in this book — the pluggable arithmetic that turns the same rules into costs and probabilities, the layered negation, the self-consistent choice models, the GPU story — is this idea, compounded.

Which leaves the question you should be asking: if it's this simple, this old, and this good, why isn't the world already written in it? Fifty years of programmers were not fools. Something went wrong — twice, expensively, once with a superpower's national budget behind it. That story is next, and Strata-K makes no sense without it.

> **Try it.** Clone the repo, then:
> ```
> cargo run -p strata-cli -- run examples/book/ch01-ownership.strata
> ```
> Add `owns(dunlin, apex).` — a real circular structure — and run again. Note what does *not* happen.
