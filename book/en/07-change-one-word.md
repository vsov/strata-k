# Chapter 7 — Change One Word

*Phase-0 status: everything in this chapter runs today — `Trop` natively, `?prob` by the exact reference method, and `?grad` alongside it: the pedigree run backward, the gradient of a query's probability with respect to every soft fact. The scaled probabilistic machinery is described where it differs; chapter 11 places it on the road.*

The trading house doesn't only ask yes-or-no questions. The logistics desk doesn't ask *whether* goods can move from Rotterdam to Tallinn — of course they can — it asks what the cheapest way is. The risk desk doesn't ask *whether* trouble at Apex can reach Dunlin — chapter 6 answered that — it asks how *likely* it is. Same relations, same recursive structure, different arithmetic.

The imperative playbook at this point is grim: the boolean reachability code, the shortest-path code, and the risk-propagation code are three unrelated programs — Dijkstra's algorithm shares no line with your graph walk — maintained by three people, drifting apart from the day they're written. This chapter makes an unreasonable-sounding claim instead: **they are one program.** The difference between them is, almost literally, one word — and the "almost" is where the chapter's real lesson lives.

## Costs: the same rules, priced

Here is the logistics network — `examples/book/ch07-routes.strata`:

```
domain hub.

pred leg(hub, hub): Trop.     % direct transport legs, cost per unit
pred route(hub, hub): Trop.   % best achievable cost, any number of legs

route(X, Y) :- leg(X, Y).
route(X, Z) :- leg(X, Y), route(Y, Z).

4 :: leg(rotterdam, gdansk).
8 :: leg(rotterdam, riga).
3 :: leg(gdansk, riga).
5 :: leg(riga, tallinn).
9 :: leg(gdansk, tallinn).
```

Two changes from anything in chapter 6. The annotation in the signatures says `Trop` instead of `Bool`. And facts now carry a number: `4 :: leg(rotterdam, gdansk)` — this leg exists *at cost 4*. The two rules defining `route` are chapter 1's transitive closure, unmodified, character for character.

```
$ strata run examples/book/ch07-routes.strata
leg(rotterdam, gdansk) = 4
...
route(rotterdam, gdansk) = 4
route(rotterdam, riga) = 7
route(rotterdam, tallinn) = 12
route(gdansk, riga) = 3
route(gdansk, tallinn) = 8
route(riga, tallinn) = 5
```

Look at `route(rotterdam, riga) = 7`. There is a direct leg at cost 8 — and the engine reports 7, having noticed that going through Gdańsk (4 + 3) beats flying direct. `route(rotterdam, tallinn) = 12` is the three-leg chain 4 + 3 + 5, beating both alternatives (13 either way). Nobody wrote Dijkstra. Nobody wrote anything — the *rules didn't change*. What changed is what the engine does when derivations meet.

## What the word actually switches

Under `Bool`, the fixpoint's bookkeeping is: a chain of conditions is an *and*; multiple derivations of the same fact are an *or*; and the values are true/false. Under `Trop` — the name is *tropical*, a mathematician's in-joke you can safely ignore — the bookkeeping is: along a chain, **add** the costs; when derivations meet, **take the minimum**. That pair of operations is the entire difference. Reachability and shortest-path were never two algorithms; they were one algorithm with two accounting policies:

| | along a chain (⊗) | where derivations meet (⊕) | values |
|---|---|---|---|
| `Bool` | and | or | true/false |
| `Trop` | + | min | costs (and +∞ for "no route") |

A structure like this — values, a "combine along the way" operation, a "merge alternatives" operation, obeying a handful of sanity laws — is called a **semiring**, and it is the taught concept of this chapter, the "pluggable arithmetic of inference" promised since chapter 5. The rules define the *shape* of inference: which facts combine, which conclusions compete. The semiring defines what flows through that shape. One program, many meanings — this is requirement four from chapter 5 arriving in the language, and it is the direct, working descendant of the provenance-semiring theory chapter 5 credited.

Two footnotes with teeth, both enforced rather than advised. Convergence: the fixpoint under `Trop` still terminates, because `min` can only improve a finite number of times — but *negative* cost cycles would let a route improve forever, so cycle detection is the engine's mandated job, not the user's assumed discipline. And the aggregate carve-out promised in chapter 6: `min` in recursion is legal under `Trop` precisely because minimum *is* this semiring's merge operation — the aggregate and the arithmetic are the same object, which is why that one aggregate escapes the between-strata rule.

If your suspicion is up — "swap the arithmetic, keep the engine" sounds too clean — good. It is too clean. One more word-swap and it breaks.

## Probabilities: the same move fails

After a trick that clean, you can see the next move from here. Costs worked by swapping one word — surely probabilities are one more swap: multiply along the chain, or-combine where derivations meet. Every textbook formula is standing by.

Let's walk into it with open eyes, on the risk desk's actual question. Apex has a 50% chance of being exposed to Brightwater — a disputed guarantee, say, that stands or falls with one court ruling. From Brightwater, contagion has two onward routes to Dunlin: directly (75%), or through Cobalt (75%, then a certain link). What is the chance trouble at Apex reaches Dunlin?

The one-more-swap arithmetic says: multiply along each route — route one: 0.5 × 0.75 = 0.375; route two: 0.5 × 0.75 × 1.0 = 0.375 — then combine the two routes: 1 − (1 − 0.375)(1 − 0.375) = **0.609375**.

Look at that number until it bothers you. Every route from Apex to Dunlin — both of them — crosses the same first link: the disputed guarantee, the one that exists with probability 0.5. If the court ruling goes the other way, there is no exposure at all, through anything. The chance of contagion reaching Dunlin cannot possibly exceed 0.5 — and we just computed 0.609 with textbook formulas and a straight face. This is not a rounding problem. The arithmetic answered a different question than the one we asked.

The bug has a name — **double counting** — and you have met it before, most likely in an availability review. Service A is 99.9% available, B is 99.9%, so A-or-B fails only when both do: 99.9999%! Except A and B run in the same rack, and the number evaporates with one power supply. The combining formula is honest only for *independent* witnesses, and our two contagion routes are not independent witnesses — they are one witness, the disputed guarantee, wearing two hats. Combining as the facts flow treats every route as fresh evidence; shared evidence gets counted once per route it appears in; every extra count inflates the answer.

Now the sharper question: why didn't this bite us with costs? Routes shared links there too — and the answer was exactly right. Because `min` is immune by nature: consider the same route twice, or ten times, and the minimum doesn't move. Counting duplicate evidence is *free* under an arithmetic that only keeps the best, and poison under an arithmetic that accumulates. That one word in the signature was doing more work than it seemed — and "does this arithmetic forgive repetition?" turns out to be the exact line between the numbers you may compute *as the facts flow* and the numbers you may not.

## The two modes, and the marked question

Strata-K draws that line in the type system, and refuses to let you cross it silently.

Arithmetics that forgive repetition — `Bool`, `Trop`, and their relatives — ride *inside* the fixpoint: the value is one more column on the fact, updated on the fly. The engine calls this **mode A**, and everything before this section ran in it.

Probability is **mode B**, and mode B changes regime entirely: during the fixpoint the engine computes no probabilities at all. Instead it records, for every derived fact, its *pedigree* — the full structure of which base facts, combined by which rules, support it, shared evidence and all. Chapter 1 introduced that structure as provenance, the answer to "why was this trade blocked?". Here provenance stops being documentation and becomes the computation itself: the pedigree of the queried fact is compiled into a circuit that counts each base fact exactly once, no matter how many derivations it appears in, and the probability falls out of the circuit, correct by construction.

You ask for it with a marked question — `examples/book/ch07-shared-evidence.strata` ends with one:

```
0.5 :: exposed(apex, brightwater).    % the shared link
0.75 :: exposed(brightwater, dunlin). % route 1
0.75 :: exposed(brightwater, cobalt). % route 2, first hop
exposed(cobalt, dunlin).              % route 2, certain hop

?prob hit(apex, dunlin).
```

```
$ strata run examples/book/ch07-shared-evidence.strata
0.46875 :: hit(apex, dunlin)
```

Below 0.5, as sanity demands: the court ruling gates everything, and behind the gate the two routes together deliver 0.9375. Half of that is the honest answer — the naive figure overstated it by nearly a quarter. Overstated contagion risk errs safe, this once; the same double-count sitting under a *netting* rule errs the other way. Pick which of your systems you'd rather have wrong.

Why mark the question as `?prob` instead of just answering? Because the circuit is not free. For pedigrees that tangle badly, compiling one is genuinely, irreducibly expensive — this is the field where "how hard is it to count without double-counting" is a famous hard problem, and no engineering makes the worst case vanish. So the language keeps the CP autopsy's promise, requirement five: cheap questions look cheap, expensive questions look expensive, and the expensive one is where you'll attach a budget when the scaled engine arrives (chapter 11 shows the declared-approximation escape valve — *top-k pedigrees*, borrowed with attribution from Scallop — whose guarantee is honest: a lower bound that only tightens).

One more enforcement, easy to miss and deeply typical of this language: `Trop` and probability *do not mix*. There is no honest exchange rate between a cost and a likelihood — no formula turns "cost 7" into "70% likely" without you asserting one — so the language refuses the combination: ask `?prob` of anything derived through a `Trop` predicate and the Phase-0 reference stops you with a named refusal at the query (the full type system, specified but not yet the enforcer, makes it a compile error at the rule site); when you want a conversion, you will write it as an explicit, named function that owns its assumptions. Where the mathematics has no bridge, the language refuses to paint one; every approximation and every conversion is declared, or it doesn't happen. You have now seen the language's second invariant in action twice; it will not be the last time.

## The aggregate corollary

Chapter 6 left a promissory note in its aggregates section — `count` and `sum` live *between* strata, never inside a recursive loop, with one carve-out to be explained here. This chapter has quietly supplied everything the explanation needs, so let's collect the debt; it takes three short experiments, two of which end in instructive refusals.

First experiment: count something across the arithmetic line. It seems innocent — "how many destinations can each hub reach?" — a `Bool`-flavored summary of a `Trop`-flavored relation:

```
pred options(hub, int): Bool.
options(X, count<Y>) :- route(X, Y).
```

```
error[E1007]: `route` (Trop) cannot flow into `options` (Bool); use an explicit conversion
```

The no-silent-crossing law you just met on the probability side turns out to guard *every* border between arithmetics, aggregation included — and this direction is refused for its own good reason: `route` facts carry costs, and "count them into a plain boolean world" silently *discards* the costs, which is a decision the language insists you make out loud. (Going the other way, recall, is free: `Bool` embeds into any arithmetic honestly, a fact at cost zero, a truth with probability one.)

Second experiment: let a count feed its own recursion — some "how many firms sit below X, counting transitively" formulation. The checker stops you with the same error code that rejected chapter 6's liar paradox, and the kinship is not cosmetic:

```
error[E1002]: predicate `deep` depends on its own negation/aggregation through a cycle
```

Here is the deep reason, and it is this chapter's lesson wearing a third costume. Ask why a self-feeding count is dangerous: the fixpoint re-derives facts freely — chapter 1 told you duplicate derivations are harmless — and a count that can see its own output counts *its own repetitions*. Sound familiar? **Non-idempotent accumulation inside the fixpoint** — the exact disease of the naive probability, wearing an integer instead of a fraction. `min` was immune, probabilities were poisoned, and `sum`/`count` are poisoned the same way. The language's response is graded by severity, and the grading *is* the design: arithmetics that forgive repetition ride the fixpoint (mode A); those that don't but have a sound alternative route go through the pedigree (mode B, `?prob`); and those where v1 has no honest story — recursive counting among them — are refused outright, by name, at compile time, rather than shipped with a footnote. Three answers, one criterion, no improvisation.

Third experiment, the carve-out, and now it's one sentence: `min` in recursion under `Trop` is legal *because minimum is this semiring's merge operation* — the aggregate and the arithmetic are the same object, so "aggregate inside the loop" and "run the fixpoint" mean the same thing. What looked in chapter 6 like an arbitrary exception is the table at the top of this chapter, read back at you.

## One program, priced three ways

Step back to the chapter's unreasonable claim: reachability, cheapest route, contagion probability — one shape, three arithmetics, and the language now owes you nothing it hasn't shown. The rules stayed fixed while the meaning moved: that's the semiring dividend. The meaning that *couldn't* move honestly was stopped at compile time: that's the double-counting line. And the question that costs more than a lookup is spelled differently: that's legibility of cost. The trading house has its logistics desk and its risk desk running on the compliance desk's rules.

What it doesn't have yet is a way to *decide* anything — every number so far describes the world as it is. The next chapter hands the language its first genuinely hard power: choosing.

> **Try it.**
> ```
> cargo run -p strata-cli -- run examples/book/ch07-routes.strata
> cargo run -p strata-cli -- run examples/book/ch07-shared-evidence.strata
> ```
> Then: (1) drop the Gdańsk–Riga leg to cost 2 and watch the improvement propagate through both downstream routes; (2) in the risk file, make the two onward routes share *another* link and check that the naive formula's error grows while `?prob` stays sane; (3) change `0.5` to `1.0` on the shared link and confirm the answer becomes exactly the two-route combination 0.9375.