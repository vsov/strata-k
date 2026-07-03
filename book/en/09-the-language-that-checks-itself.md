# Chapter 9 — The Language That Checks Itself

*Phase-0 status: every tool invocation in this chapter — `check`, `run`, `fmt`, `ir` — is real and reproducible from the repository. The LLM transcript is illustrative: it was produced by a current model (July 2026) from the prompt shown, but models are not deterministic and yours may err differently. That variability is not a caveat undermining the chapter — it is the chapter's entire point. Nothing below depends on the model being right.*

Part I ended on an inversion: for fifty years languages were optimized for the writer, and the LLM era flips the scarce resource from *writing* code to *trusting* code someone — something — else wrote. Every design decision in the last three chapters was made with that inversion in view. This chapter finally runs the experiment: hand the keyboard to the machine, and watch which properties of the language carry the weight.

## The vignette

The compliance desk receives a new mandate, in English, the way mandates actually arrive:

> "Do not clear a trade if any beneficial owner of the counterparty, within two levels of ownership, is on the sanctions list."

An engineer pastes the sentence into a model, along with the trading house's predicate declarations, and asks for Strata-K. Back comes a draft — `examples/book/ch09-vignette-draft.strata`:

```
domain firm.

pred owns(firm, firm): Bool.
pred listed(firm): Bool.
pred counterparty(firm): Bool.
pred owner_within2(firm, firm): Bool.
pred blocked(firm): Bool.
pred cleared(firm): Bool.

owner_within2(P, C) :- owns(P, C).
owner_within2(P, C) :- owns(P, M), owns(M, C).

blocked(C) :- counterparty(C), owner_within2(P, C), listd(P).

cleared(C) :- counterparty(C), not blocked(C).

owns(apex, brightwater).
owns(brightwater, dunlin).
listed(apex).
counterparty(dunlin).
```

Read it before the tools do — the model has done real work here. It noticed "within two levels" means *bounded* recursion and correctly wrote `owner_within2` as two non-recursive rules rather than copying chapter 6's unbounded `controls`; it kept the stratified shape; the test facts even exercise the two-level case. It also, in the `blocked` rule, spelled `listed` as `listd`.

Now run the moment this book has been building toward since the design note in chapter 6. In Prolog, in an expert shell, in classical Datalog, `listd` is a legal predicate that happens to have no facts: `blocked` silently never fires, and the program *clears every counterparty on the sanctions list* — the worst possible failure, deployed with green tests unless a test happens to cover the typo. In Strata-K:

```
$ strata check examples/book/ch09-vignette-draft.strata
error[E1001]: predicate `listd` is used but never declared
  --> 16:1
   | blocked(C) :- counterparty(C), owner_within2(P, C), listd(P).
   | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

A stable error code, the offending line, the span. Feed exactly that text back to the model — this is the repair loop from chapter 5's line five, and the error message is its fuel — and the fix comes back one token wide. `strata check` now reports `ok`; then:

```
$ strata run examples/book/ch09-vignette.strata
blocked(dunlin)
counterparty(dunlin)
listed(apex)
owner_within2(apex, brightwater)
owner_within2(apex, dunlin)
owner_within2(brightwater, dunlin)
owns(apex, brightwater)
owns(brightwater, dunlin)
```

The engineer's last step is the English gloss, and it writes itself off that fact list, because the derivation behind the verdict is three facts long: *dunlin is blocked because apex owns brightwater and brightwater owns dunlin — so apex is an owner within two levels — and apex is listed.* Hold that against the original mandate sentence. An auditor can check the one against the other clause by clause, which is the property the prologue said a regulated industry is entitled to and imperative code cannot give: not a diff of implementations, but a correspondence between a *rule as stated* and a *conclusion as derived*.

That is the whole loop: requirement in, rules out, defect caught by the checker rather than the production incident, repair driven by a named error, result explained in the vocabulary of the requirement. One pass through it is anecdote; what makes it an argument is *why* each step held. So — the property walk.

## Why the typo could not survive

Mandatory signatures, chapter 6's design note, now with the payoff banked. Note precisely what saved the run: not model intelligence, and not test coverage — the *grammar of the language* made "predicate that exists nowhere" unsayable. LLMs are pattern engines; their characteristic failure is the plausible near-miss — `listd`, a swapped argument pair, an arity off by one — and this language was shaped so that the plausible near-miss class lands, structurally, in compile errors instead of silent semantics. You cannot make a model stop typo-ing. You can make a language in which typos have nowhere to hide.

## The error the checker cannot see

Honesty demands the second act, because the first one can leave a false impression: that `strata check` is the safety net. It is the *first* net. Here is a draft the model could just as plausibly have produced — identical to the repaired program except for one rule, where the arguments of `owner_within2` arrive swapped:

```
blocked(C) :- counterparty(C), owner_within2(C, P), listed(P).
```

Both arguments are firms; the types match; the arity matches; nothing is undeclared. `strata check` reports `ok` — as it must, because this program is perfectly well-formed. It just answers a different question: "is C blocked when C *owns someone* who is listed" instead of "when someone who owns C is listed." Ownership, upside down. Run it:

```
$ strata run examples/book/ch09-vignette-draft2.strata
cleared(dunlin)
...
```

Dunlin — owned, two levels up, by a sanctioned firm — walks free. This is the plausible near-miss in its most dangerous form: past the type checker, wearing green.

What catches it is the next layer down, and the mandate itself supplied the net: every compliance requirement arrives with worked examples, or can be made to — "Apex is listed, Apex owns Brightwater owns Dunlin, therefore Dunlin must be blocked" is precisely such an example, and it was sitting in the draft's own test facts. Assert it — *expect `blocked(dunlin)`* — and the swapped version fails loudly. The crucial point is *why* this test is trustworthy here in a way integration tests never quite are in imperative code: the language is deterministic and order-free, so when the expectation fails there is exactly one suspect — the rules. Not flaky scheduling, not test-ordering, not a stale cache: the rules. A disagreement between a worked example and a fixpoint is a *fact about the program*, and the repair loop can consume it just as it consumed `E1001`.

So the verification stack is layered, and each layer catches what the one above cannot: the schema catches malformed structure, the signatures catch the unsayable, stratification catches the paradoxical, and worked examples catch the well-formed lie. Layer four exists in every language, of course — it's called testing. What the language contributes is that layer four here is *cheap and conclusive*: milliseconds to run, deterministic to interpret, with the pedigree (chapter 7) available to show exactly which rule chain produced the wrong verdict. The draft's swap, unfolded, reads "dunlin is cleared because no owner of dunlin is listed — wait, the rule asked whom *dunlin* owns" — and the bug explains itself in the mandate's own vocabulary.

## Why order never entered the conversation

The model emitted declarations, rules, and facts in whatever order its sampling produced. Nobody checked; nobody needed to. In the language of chapter 4's autopsies: Prolog's meaning-depends-on-order would have made that same generation a lottery ticket — recall that reordering two logically identical clauses produced the silent forever-hang, the one failure mode a repair loop cannot even *see*, because there is no error text to feed back. Order-independence converts a whole class of generation accidents into non-events. It also makes program *assembly* safe: rules generated today concatenate with rules generated last sprint, in any order, and the fixpoint is the fixpoint.

## Why the review was readable

The unit the human reviewed was a rule — one sentence, readable aloud, checkable against one clause of the mandate. That granularity is a language property, not a model property: the grammar has no long-range state, no rule can reach into another, and each rule's meaning is complete on its face given the signatures. Compare the review unit you're used to: a 40-line method where the mandate's clause 2 is smeared across a guard, a cache invalidation, and an early return. The language's smallness — a couple dozen productions, no methods, no inheritance, no lifecycle — is not primitivism; it is the same design pressure that made the *checkable* unit and the *stated* unit coincide.

There is a second face to this smallness, machine-facing rather than human-facing. Every Strata-K program has one canonical spelling — `strata fmt` is not a style vote, it is *the* projection — so diffs are semantic, never cosmetic, and a model regenerating a file cannot flood review with whitespace noise. And underneath the surface syntax sits the actual source of truth: a JSON document (the "High-IR"), schema-published, that the surface merely renders. `strata ir` converts both ways:

```
$ strata ir examples/book/ch09-vignette.strata --to json
{
  "ir_version": "0.1.0",
  "items": [
    { "kind": "domain",    "data": { "name": "firm" }, ... },
    { "kind": "predicate", ... },
    ...
```

A model — or a pipeline — can skip surface syntax entirely and emit schema-validated IR, with structural validity guaranteed by the schema check before the type checker even wakes. Two authoring formats, one meaning, mechanical round-trip: the language meets its machine authors on their terms, and its human reviewers on theirs.

## Why the loop converges

Every diagnostic carries a stable code (`E1001` is `E1001` forever — prompts, docs, and tooling can rely on it), a span, and, where the fix is mechanical, a machine-applicable suggestion. The check itself is total and deterministic: same file, same verdict, every time, in milliseconds — which means the repair loop's feedback signal is *clean*. Contrast the feedback signals chapter 4 catalogued: a hang (no signal), a bare `UNSATISFIABLE` (a signal with no gradient), a wrong-but-plausible answer (a signal you discover in production). A generation loop is a control system; it converges when errors are prompt, local, and named, and diverges when they are late, global, and mute. That single sentence is most of Strata-K's LLM story, and not one word of it required the model to be good.

And the loop has a floor the model cannot fall through — you watched it hold, two sections ago, when the swapped-argument draft sailed past `check` and was stopped by a three-fact worked example. The stack — schema, types, stratification, examples, then the human reading the rules that survived — is cheap at every layer because the language was shaped to keep it cheap, and each layer's verdict feeds the same repair loop.

## The division of labor, stated plainly

Notice what was never claimed in this chapter. Not once did the language make the model smarter, and not once did the argument depend on trusting the model. The claim is the constitution's third invariant, wearing its LLM clothes: **nothing soft decides; soft things propose, and the symbolic core disposes.** The model proposes rules; the checker and the fixpoint dispose. In chapter 11 the same sentence reappears with "neural network" in place of "LLM" and the trading house's data flowing through it — it is one principle, and this chapter was its cheapest, most reproducible demonstration.

The demonstration also quietly used up the last of the language's introduced-but-unproven claims. Chapters 6 through 8 showed the three regimes; this chapter showed the authorship story they were shaped for. What remains is the part of the design you cannot run today — the engine the core was shaped *for*, and the road that gets there. Two chapters: the iron, and the road.

> **Try it.** Reproduce the whole vignette:
> ```
> cargo run -p strata-cli -- check examples/book/ch09-vignette-draft.strata   # the catch
> cargo run -p strata-cli -- run  examples/book/ch09-vignette.strata         # the repaired run
> cargo run -p strata-cli -- ir   examples/book/ch09-vignette.strata --to json | head -20
> ```
> Then go one better than the transcript: paste the mandate sentence and the `pred` declarations into the model of your choice, and run *its* draft through `strata check`. Your error will differ from mine. The loop won't care — that's the point.