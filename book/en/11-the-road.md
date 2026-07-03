# Chapter 11 — The Road

*Status: this chapter is the one the status boxes have been deferring to. Everything in it is design or vision, explicitly staged; the only executable things here are the ways today's tools already acknowledge tomorrow's syntax.*

Every claim in chapters 6 through 9 you could run. Chapter 10 was design with an oracle waiting for it. What remains is the part of the project that is honestly a bet — and the prologue promised that when this book speculates, it says so. This chapter says so. It also says something rarer: exactly *which* pieces are settled engineering, which are designed-but-unbuilt, and which are open research, because those are three different kinds of promise and mixing them is how fields earn winters.

## The boundary where the network lives

Start with the piece the whole design has been shaped around and has never yet shown: the neural predicate. Try it today:

```
$ strata check examples/book/ch11-neural.strata
error[E0100]: neural predicates is not implemented in Phase 0
  --> 3:1
   | neural flag(firm, label) from model "aml_gnn".
   | ^^^^^^
```

Read that diagnostic carefully, because it is doing something unusual. The declaration *parsed* — the grammar of the full language, neural predicates included, is already the shipped grammar, and unbuilt features are refused by name with a stable code, not rejected as gibberish. The surface is complete; the execution is staged. That is what a future-syntax frame means mechanically, and it is a small design decision with a purpose: programs written against the full language today fail loudly and specifically, never silently and confusingly — the same courtesy the language extends to typos, extended to its own roadmap.

Here is what the declaration will mean, and it costs one sentence because every concept in it is already yours: `flag` is a predicate whose facts arrive not from the file but from a model's inference — each fact annotated with the model's confidence, exactly as chapter 7's probabilistic facts were annotated — flowing into rules that treat it like any other soft evidence, behind the same mode-B line, with the same taint discipline: anything derived from it is marked soft in its signature, forever, by the type system. **The network proposes; the rules dispose.** An anti-money-laundering screen in this style is: a graph model flags suspicious structures with confidences; hard rules — the kind chapters 6 and 8 taught — take the flags as probabilistic input and derive `investigate(X)` with a pedigree; the pedigree distinguishes "flagged by the model" from "derived from ownership facts" *in the type of the conclusion*. The auditor sees which part of the answer is theorem and which part is opinion. No fragment in chapter 3, and no system in chapter 5 short of the research frontier itself, offers that sentence.

And the gradient flows back. The pedigree circuit of chapter 7 is differentiable — run backward, it tells the model *how much each of its confidences mattered to the conclusion* — so the training loop of the host framework can teach the network through the logic. That is the design's participation in the neuro-symbolic convergence of chapter 5, borrowed pieces credited there; `?grad`, parsed and staged like everything else, is its query form.

## The phases, and the kind of promise each one is

The road from the repository you can clone to the system of chapters 10–11, in the order the engineering dependencies dictate — with each phase labeled by promise-kind: **[E]** settled engineering (the field knows how; it's work), **[D]** designed here, unbuilt (chapter 10's kind — an oracle exists to hold it honest), **[R]** open research (no one knows; declared as such).

1. **Scale the pedigree machinery.** Circuit compilation for exact probabilistic queries beyond toy size **[E — the knowledge-compilation field's standard toolbox]**, and top-k pedigrees for recursive programs, the declared lower-bound approximation credited in chapter 5 **[E, borrowed with attribution]**.
2. **Incrementality.** New facts arrive, conclusions update without recomputation — the differential machinery chapter 5's line three proved out, applied to this language **[E in the literature, D in this design]**. For the trading house this is the difference between a nightly batch and a compliance engine that answers *during* the trade.
3. **The GPU engine.** Chapter 10, executed: columnar fixpoint, hybrid planner, the three-processor split **[D, oracle in hand, related work already demonstrating the direction]**.
4. **The neural boundary.** Predicates from models, gradients back through pedigrees **[D at the interface; E for the pieces it borrows; the *integration at production scale* honestly sits between D and R]**.
5. **Structured values** — `@terms`, the fenced extension that trades away guaranteed termination for constructor terms (lists, trees) with declared-incompleteness controls **[D, and the fence is the design]**.
6. **The bridges that don't exist yet [R, plainly]:** probability across the `@asp` fence — the semantics of "likelihood over self-consistent worlds" — is an open research area, and this book has already told you (chapter 8) that the type checker refuses the combination rather than improvise it. If the field settles it, the fence gets a gate. Until then, the constitution holds: no arithmetic without a semantics.

What is deliberately *not* on the road: nothing from chapter 5's refusals has crept back. No theorem proving, no continuous-domain constraint solving, no training framework, no general-purpose ambitions. The road makes the language more of what it is, not more things.

## Where this leaves you

The prologue asked you to carry one thing through eleven chapters: your own version of the sanctions smear — the rule in your codebase that is everywhere and nowhere. Here is the whole journey, measured against it.

The rule can be *written as what it is* — chapter 1's bargain, chapter 6's thirty lines, readable by the officer who owns it. It can carry cost and likelihood without lying about either — chapter 7's one word, and the line it refused to cross. It can face genuine choice — chapter 8's fenced worlds. It can be drafted by a machine and *caught* when the draft is wrong — chapter 9's one-token repair, the loop that never asked the model to be trustworthy. It can, when the road is walked, run at the speed of the hardware your industry already owns — chapter 10's bet, honestly boxed. And at every step, the answer can show its work: the pedigree, the derivation an auditor can hold against the regulation, clause by clause. Programs that know why.

The third bet is placed in the open — the spec, the reference implementation, every listing in this book, and the differential harness that keeps the fast future honest all live in one repository, and the phases above are its issue tracker, not a prophecy. Clone it. Run the trading house. Break the checker; read its errors; feed a mandate of your own to a model and see what survives `strata check`. FGCS teaches that this paradigm's bets die of arriving before their hardware; chapter 2 argued the hardware has finally arrived, chapters 6 through 9 put the language in your hands, and the only piece of the argument no book can supply is the working engineer who tries the fifty-year-old idea on this decade's problem and decides, on the evidence, whether it holds.

That's you, and the repository is open. The rules, at last, can be the program.