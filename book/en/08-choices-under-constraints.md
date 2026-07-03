# Chapter 8 — Choices Under Constraints

*Phase-0 status: everything here runs today, on the reference answer-set solver in the repository.*

Everything the language has done so far — deriving, pricing, weighing — describes the world as it is. But half the interesting questions at a trading house are not "what follows?" but "what should we *pick*?" The portfolio desk holds a list of candidate positions and a book of mandates: certain pairs may not be held together — a client agreement forbids holding gas futures against carbon credits, say. Which combinations of positions are acceptable?

Feel how the ground shifts under that question. It has no single answer — there are many acceptable portfolios, and *enumerating the legitimate alternatives* is the deliverable. The deductive core of chapters 6 and 7 is constitutionally incapable of this, and proudly so: its whole guarantee is that the facts and rules determine one fixpoint, every derived fact a theorem. "Pick some subset, any consistent one" is not a theorem. Ask an imperative programmer and you get the third member of the zoo from chapter 1 — next to the graph walk and the worklist, there's the backtracking enumerator with the pruning conditions threaded through it, the one whose recursion nobody wants to touch.

Chapter 4 already met the paradigm built for exactly this — ASP — and found the semantics right and the walls real. So Strata-K does the only honest thing: it takes the semantics and fences it. That fence is the pragma at the top of today's file, and it is the language's third invariant made visible: guarantees by default, escapes explicit and marked. Outside the fence, the deductive world of one-true-fixpoint. Inside, a different physics.

## The program

`examples/book/ch08-portfolio.strata`:

```
@asp.
domain pos.

pred candidate(pos): Bool.
pred take(pos): Bool.
pred skip(pos): Bool.
pred conflict(pos, pos): Bool.
pred viol(): Bool.

% Each candidate position is independently taken or skipped.
take(P) :- candidate(P), not skip(P).
skip(P) :- candidate(P), not take(P).

% Mandates: these pairs may not be held together.
conflict(gas_futures, carbon_credits).
conflict(carbon_credits, gas_futures).

% No self-consistent world may hold a conflicting pair.
viol() :- take(X), take(Y), conflict(X, Y), not viol().

candidate(gas_futures).
candidate(carbon_credits).
candidate(fx_swap).
```

Stop at the pair of rules defining `take` and `skip`, because chapter 6 trained you to reject them on sight: `take` depends on the negation of `skip`, `skip` on the negation of `take` — a cycle through `not`, exactly what the stratification checker rejects programs for. Inside `@asp`, this shape is not an accident to be rejected; it is *the* idiom, and what it expresses is choice.

## Stable models: self-consistent worlds

Here is the semantics, in the plain terms this book promised. A **stable model** is a set of conclusions that earns its keep in both directions: take the set as given — treat every `not q` in the rules as settled by whether `q` is in the set — and re-derive everything the rules then produce. If you get back *exactly the set you assumed* — nothing missing, nothing extra, every member re-derivable — the set is stable: a self-consistent world. If the assumption produces more than it assumed, or fails to reproduce a member, it was never a coherent world at all, just a wrong guess.

Run the `take`/`skip` pair through that definition for one candidate, `fx_swap`. Assume a world where `take(fx_swap)` holds and `skip(fx_swap)` doesn't: then `not skip(fx_swap)` is settled-true, the first rule re-derives `take(fx_swap)`, the second is blocked — the world reproduces itself exactly. Stable. The mirror-image world (`skip`, no `take`) is stable by symmetry. But a world with *neither*: both `not`s are settled-true, both rules fire, and the re-derivation produces both facts where zero were assumed — not a world, a contradiction. And a world with *both* fails the other direction: each rule is blocked by the other's conclusion, nothing is re-derivable, yet the assumption claimed both. The two-rule cycle is therefore a fork with exactly two self-consistent resolutions per candidate — an "or" that the one-fixpoint core could never speak. Three independent candidates: 2 × 2 × 2 = eight candidate worlds.

The `viol` rule then kills the bad ones, and it works by weaponizing the liar's paradox from chapter 6. Read it again: if a conflicting pair is held and `viol()` is *not* in the world, the rule derives `viol()` — the assumption fails to reproduce itself. If you instead assume `viol()` *is* in the world, the `not viol()` in its own body blocks the only rule that could derive it — and an underivable assumption is also unstable. Heads you lose, tails you lose: **no stable world can hold a conflicting pair.** The paradox that the deductive core rejects at compile time becomes, inside the fence, a scalpel for cutting worlds away. (I'll be honest about ergonomics: production ASP dialects spell this pattern with dedicated constraint syntax rather than an explicit paradox loop, and surface sugar for it is on Strata-K's roadmap — what you see here is the semantic mechanism itself, undisguised.)

```
$ strata run examples/book/ch08-portfolio.strata
Answer 1: {..., skip(carbon_credits), skip(fx_swap), skip(gas_futures)}
Answer 2: {..., skip(carbon_credits), skip(fx_swap), take(gas_futures)}
Answer 3: {..., skip(carbon_credits), skip(gas_futures), take(fx_swap)}
Answer 4: {..., skip(carbon_credits), take(fx_swap), take(gas_futures)}
Answer 5: {..., skip(fx_swap), skip(gas_futures), take(carbon_credits)}
Answer 6: {..., skip(gas_futures), take(carbon_credits), take(fx_swap)}
```

(Each answer also lists the `candidate` and `conflict` facts — elided here as `...` for the page; run it to see them.) Six worlds, not eight: the two containing both gas futures and carbon credits are gone, and every mandate-respecting portfolio — including the empty one, a legitimate if unambitious choice — is present exactly once. The deliverable *is* the enumeration: hand the six to the desk, or pipe them into downstream rules that score them.

## What the fence buys, in both directions

Notice what stayed and what changed at the `@asp` boundary — the trade is precise, not rhetorical.

**Stayed:** the entire linguistic frame. Same syntax, same signatures with the same compile-time protection, same reading-aloud discipline (`take P if P is a candidate and you don't skip it` — the rules remain sentences). Same order-independence: shuffle the file, same six answers. Termination, too, survives — the fence encloses finite worlds over declared domains, not unbounded search.

**Changed:** the meaning of a run. Outside the fence, a program denotes one world and every fact in it is a theorem; inside, it denotes a *set of worlds*, and the honest questions become "in some acceptable world?" / "in every acceptable world?" — which is exactly the vocabulary a compliance officer already uses ("is there *any* compliant allocation?"; "must we hold this in *all* of them?"). And the cost model changed teeth: finding stable models is genuinely combinatorial — this is the ground where chapter 4 measured the grounding cliff, and the fence does not repeal it; the reference solver behind today's listing is built for correctness, not scale (chapter 10 says what division of labor the scaled architecture assigns, and to which processor). The pragma at the top of the file is thus a price sticker in exactly the `?prob` tradition: you can *see* at the first line that this module buys expressive power with search.

**Refused:** the mixture. Probabilistic facts do not enter `@asp` modules — not because the combination isn't tantalizing (it is, and chapter 11 touches the research frontier growing toward it) but because the mathematics of "probability over self-consistent worlds with cyclic negation" is not settled ground the way everything else in this book is, and the constitution forbids shipping an arithmetic without a semantics. Where chapter 7's line was "no silent conversion," this chapter's is starker: no bridge at all, yet, and the type checker says so out loud.

## The shape so far

Take stock of the language you now hold, because with this chapter its expressive frame is complete. A deductive core where everything is a theorem (chapter 6). Pluggable arithmetic over that core, with the double-counting line enforced between the arithmetics that ride the fixpoint and the ones that need the pedigree (chapter 7). And a fenced yard for genuine choice, where the semantics switches from *the* world to *the acceptable* worlds (this chapter). Three regimes, three visibly different price stickers, one syntax — and each regime's guarantee stated on its own terms rather than averaged into mush.

What the language has not yet had to face is its intended *author*. Every program so far was written by me, slowly, for you. The next chapter hands the keyboard to the machine the whole design has been bracing for — and shows what the bracing was worth.

> **Try it.**
> ```
> cargo run -p strata-cli -- run examples/book/ch08-portfolio.strata
> ```
> Then: (1) add a third mandate — `conflict(fx_swap, gas_futures).` and its mirror — and predict the answer count before running (you should get five); (2) delete the `viol` rule and confirm all eight worlds return; (3) add `candidate(rates_basis).` and watch the enumeration double.