# Chapter 6 — Meet Strata-K

> **Status — read this first.** Everything you run in chapters 6 through 9 executes today, on the CPU reference implementation in the book's repository: parser, type checker, evaluator, exact probabilistic queries and their gradients, structural terms, answer-set solver — cross-checked against the independent Soufflé engine on the shared fragment, and fuzzed against itself over tens of thousands of random programs. The GPU engine of chapter 10 has since been built and is validated bit-for-bit against this reference (its measurements live in the repository, not in these pages); chapter 11's neural boundary runs at reference scale. What still does *not* execute: the full-pedigree `Prov` annotation — the one construct left inside a "future syntax" frame, which parses today and executes in a later phase. Everything else runs, and every output printed in this book is the tool's real output. That's the deal, and it holds to the last page.

Part I ended with six requirements and an empty table. Time to put something on it — not the whole language at once, but its deductive core, on a worked example sized like the real world's problems and small enough to hold in your head. This chapter's job is for you to *read* a complete Strata-K program the way its checker does, and to see the first three requirements — parallel-ready core, order-independence, guarantees by default — fall out of decisions you can point at in the source.

## The trading house

Meet the world every remaining chapter builds on. A trading house faces a web of counterparties — firms it buys from, sells to, clears through. Firms own stakes in other firms. A regulator publishes a sanctions list. And the rule from the prologue applies in its full inconvenience: a counterparty is untouchable if *anyone who ultimately controls it* is listed.

Here is the entire compliance kernel, as a program — `examples/book/ch06-trading-house.strata`:

```
domain firm.

pred owns(firm, firm): Bool.        % direct ownership stakes
pred controls(firm, firm): Bool.    % beneficial control, any depth
pred listed(firm): Bool.            % appears on the sanctions list
pred counterparty(firm): Bool.      % firms we actually trade with
pred blocked(firm): Bool.           % barred by the sanctions rule
pred cleared(firm): Bool.           % free to trade
pred stake_count(firm, int): Bool.  % how many direct stakes a firm holds

% The regulation, as rules.
controls(X, Y) :- owns(X, Y).
controls(X, Z) :- owns(X, Y), controls(Y, Z).

blocked(C) :- counterparty(C), listed(C).
blocked(C) :- counterparty(C), controls(P, C), listed(P).

cleared(C) :- counterparty(C), not blocked(C).

stake_count(X, count<Y>) :- owns(X, Y).

% The world as we know it today.
owns(apex, brightwater).
owns(apex, cobalt).
owns(brightwater, dunlin).
owns(cobalt, eiders).
owns(fenwick, gullwing).

listed(apex).

counterparty(dunlin).
counterparty(eiders).
counterparty(gullwing).
```

Thirty-odd lines, three sections — declarations, rules, facts — and you have already met most of the ideas in Part I. Let's read it top to bottom, stopping wherever a design decision is doing work.

## Declarations: the part Datalog never had

The block of `pred` lines is the first thing a Datalog veteran would notice, because classical Datalog doesn't have it: every predicate must be declared, with the domains of its arguments and an annotation after the colon. `Bool` says these are plain true-or-false facts — the only annotation this chapter needs; the next chapter is entirely about what else can stand in that position.

> **Design note — why signatures are mandatory.** In classical Datalog, Prolog, and every expert shell, referring to a predicate that doesn't exist is not an error — it is simply a predicate with no facts, silently true nowhere. Misspell `listed` in a rule and the rule quietly never fires; your sanctions check runs, passes every test that doesn't cover the typo, and clears everything. In a language meant to be written at speed by machines and reviewed by busy humans, that failure mode is disqualifying. Strata-K closes it structurally: every predicate is declared, so a misspelling is a *compile error with a location*, not an empty relation. You will watch this exact scenario play out, with real tool output, in chapter 9 — it is requirement two of chapter 5, cashed in.

The declarations also carry the types. `owns(firm, firm)` relates firms to firms; `stake_count(firm, int)` pairs a firm with a number. Arities and argument types are checked everywhere the predicate appears — one more class of silent nonsense (swapped arguments, wrong arity) converted into named, located errors.

## Rules: the regulation, executable

The rules section you can already read fluently. `controls` is chapter 1's transitive closure, verbatim. The two `blocked` rules are the regulation's two cases — directly listed, or controlled at any depth by someone listed — and note that they are *two separate rules*, not one rule with an `or`: in this language, alternatives are alternative rules. Multiple rules for the same head are how disjunction is spelled, and the engine treats every derivation route identically — you saw in chapter 1 why duplicate derivations cost nothing here.

`cleared` introduces the one genuinely new construct of this chapter, and it deserves its own section.

## Negation, stratified: computing what's absent

```
cleared(C) :- counterparty(C), not blocked(C).
```

Read it aloud: *C is cleared if C is a counterparty and C is not blocked.* Nothing looks dangerous. But `not` in a fixpoint language is a genuinely subtle guest, and the subtlety is worth two minutes, because the language's answer to it is one of its load-bearing walls.

The fixpoint of chapter 1 grows monotonically: every pass *adds* facts, and nothing ever becomes false again. `not blocked(C)` breaks that comfort — it asks about the *absence* of a fact. Absence when? If the engine checks "is `blocked(dunlin)` absent?" too early — before the `blocked` rules have finished firing — it might conclude `cleared(dunlin)`, then later derive `blocked(dunlin)`, and now the model contradicts itself. Worse, feed negation back into its own definition — `p :- not p.` — and there is no sensible answer at all: if `p` is false it must be true, if true it must be false. The liar's paradox, in one line.

Strata-K's rule is the classical one, and it is exactly the plain-English sentence from this book's glossary: **you may only negate what has been fully computed first.** The checker builds the dependency graph of predicates and splits the program into *strata* — layers. `owns`, `listed`, `counterparty` sit at the bottom; `controls` and `blocked` build on them; `cleared`, which negates `blocked`, sits in a stratum strictly above it. The engine runs each stratum to its fixpoint before the next begins, so by the time any rule asks `not blocked(C)`, the set of blocked firms is finished and frozen — the question has one answer, forever. And a program whose negation *can't* be layered — any cycle through a `not`, with the liar as the smallest case — is rejected at compile time, with the offending predicate named in the error.

Pause on the shape of that guarantee, because it is the constitution of this language in miniature (you met the idea as a refrain starting in chapter 5): a fact derived in this program is not "true as of when we checked, under the depth strategy, unless a later rule retracted it" — the hedges you'd wear in an expert shell. It is a theorem: a consequence of the stated facts and rules, unconditionally, reproducibly, in any evaluation order. The language's first invariant says exactly this — *nothing approximate may leak into a `Bool` derivation without a compile error* — and each extension in the coming chapters will be measured against it.

There is one honest limitation to flag now: some problems *want* the unstratifiable shape — not as paradox but as *choice*, where "assume it's in, unless it's out" is the actual specification. The deductive core refuses those programs. Chapter 8 opens the fenced yard where they are welcome.

## Aggregates, and running the world

`stake_count(X, count<Y>) :- owns(X, Y).` — for each `X`, count the `Y`s. The aggregate vocabulary is `count`, `sum`, `min`, `max`, and the same stratification logic governs it: aggregation, like negation, needs its input finished before it summarizes, so it lives between strata, never inside a recursive loop (one carve-out comes in the next chapter, where a particular aggregate and a particular arithmetic turn out to be made for each other).

Run the world:

```
$ strata run examples/book/ch06-trading-house.strata
blocked(dunlin)
blocked(eiders)
cleared(gullwing)
controls(apex, brightwater)
controls(apex, cobalt)
controls(apex, dunlin)
controls(apex, eiders)
...
stake_count(apex, 2)
stake_count(brightwater, 1)
stake_count(cobalt, 1)
stake_count(fenwick, 1)
```

Trace `blocked(dunlin)` yourself, once, on paper — it's three steps and it is the whole paradigm: `owns(apex, brightwater)` and `owns(brightwater, dunlin)` give `controls(apex, dunlin)` through the recursion; `listed(apex)` plus `counterparty(dunlin)` fire the second `blocked` rule. Dunlin is untouchable because of who sits two levels above it, and *nobody wrote the walk* — no depth parameter, no cache, no nightly job. Eiders falls the same way through cobalt. Gullwing, owned by unlisted fenwick, clears. The prologue's smear — recursive walk, flattening cache, depth limit, hotfix — is these thirty lines, and the depth limit that someone hard-coded as `3` out of fear is simply *gone*, because the fixpoint has no depth to limit.

And the compliance officer can read the rules. That sentence is easy to skim past, so let me weigh it: the artifact the engineers maintain and the artifact the domain expert can verify are, for the first time in this book, *the same artifact*.

## What you can no longer write

A tour of this language is incomplete without the negative space — the imperative habits that have no translation here, each one a deliberate absence:

**You cannot order anything.** There is no "first this rule, then that one." Rules fire logically simultaneously; files concatenate in any order; the answer is the fixpoint, full stop. The freedom you're giving up was never freedom — it was the hand-compilation of control that Part I spent four chapters indicting.

**You cannot update or delete.** No rule retracts a fact. Within a run, the world only grows toward its fixpoint — that's what makes derived facts theorems (and, later, what makes the engine massively parallel and the incremental story tractable). "The data changed" is handled where it belongs: new facts in, run again — and *efficiently* reusing the previous answer is the engine's job on the roadmap, not a mutation you perform by hand.

**You cannot call anything.** No side effects, no I/O in rules, no escape into a host language from inside the logic. The expert shells' original sin — rule bodies doing arbitrary things, meaning held hostage by firing order — is not restricted here; it is absent.

Each absence is a requirement from chapter 5 wearing work clothes. Determinism and order-independence aren't features the implementation strives for; they are what's left when the constructs that could violate them are removed from the grammar.

> **Try it.**
> ```
> cargo run -p strata-cli -- run examples/book/ch06-trading-house.strata
> ```
> Then: (1) add `owns(gullwing, fenwick).` — a two-firm ownership cycle — and watch `controls` close over it harmlessly; (2) misspell `listed` as `listd` in a rule and run `strata check` on the file — chapter 9 will make this error message the hero of a longer story; (3) try the liar: add `pred p(): Bool.` and `p() :- not p().` and read the stratification error.

One word of the signature — that `Bool` after the colon — has been sitting quietly through this whole chapter. The next chapter changes it, twice, and the second change breaks something important on purpose.
