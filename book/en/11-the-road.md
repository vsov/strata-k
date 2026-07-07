# Chapter 11 — The Road

*Status: this chapter is the one the status boxes have been deferring to. It was written as design and vision, explicitly staged — and much of it has since crossed onto the built side (the neural boundary, the GPU engine, the pedigree machinery down to the surface annotations, structured values). Where a piece has crossed, the chapter runs it; where it hasn't, the label still says so.*

Every claim in chapters 6 through 9 you could run. Chapter 10 was design with an oracle waiting for it — and the engine has since been built against that oracle. What remains is the part of the project that is honestly a bet — and the prologue promised that when this book speculates, it says so. This chapter says so. It also says something rarer: exactly *which* pieces are settled engineering, which are designed-but-unbuilt, and which are open research, because those are three different kinds of promise and mixing them is how fields earn winters.

## The boundary where the network lives

Start with the piece the whole design was shaped around: the neural predicate. It runs today.

```
$ strata run examples/book/ch11-neural.strata
0.9 :: investigate(acme)
  ∂/∂[0.9 :: flag(acme, structuring)] = 1  (→ model "aml_gnn")
```

Read what happened, because every concept in it is already yours. `flag` is a predicate whose facts arrive not from the file but from a model's inference — each annotated with the model's confidence, exactly as chapter 7's probabilistic facts were annotated — flowing into rules that treat it like any other soft evidence, behind the same mode-B line, with the same taint discipline: anything derived from it is soft in its signature, forever, by the type system. **The network proposes; the rules dispose.** The anti-money-laundering screen is: a graph model flags suspicious structures with confidences; hard rules — the kind chapters 6 and 8 taught — take the flags as probabilistic input and derive `investigate(X)` with a pedigree that distinguishes "flagged by the model" from "derived from ownership facts" *in the type of the conclusion*. The auditor sees which part of the answer is theorem and which part is opinion. No fragment in chapter 3, and no system in chapter 5 short of the research frontier itself, offers that.

And the gradient flows back — that second line. `?grad` runs the pedigree circuit of chapter 7 backward and reports *how much each of the model's confidences mattered to the conclusion*: here `investigate(acme)` rests entirely on the one flag, so the number is `1`, and that number is exactly what a host training loop backpropagates into the network. The design's participation in the neuro-symbolic convergence of chapter 5, borrowed pieces credited there, is a query you can run.

In this listing the *model itself* — the network whose forward pass produced `0.9 :: flag(acme, structuring)` — is supplied as data, its outputs written into the program the way a batch of inferences would be dumped. The in-process wiring, where the facts are *computed* rather than pasted, is settled engineering, and the reference now ships it as a library boundary: a model object whose forward pass runs at evaluation time and becomes the soft facts, gradients flowing back to it by position (`strata-k`, the embeddable facade — the repository's `neural_inprocess` example is this chapter's listing with the pasting removed). What remains honestly open there is *scale*, not wiring. And the fuller pedigree the design wants — the whole provenance as a compiled circuit, not just a marginal — was, for most of this book's writing, the piece one step further out. It isn't anymore. Ask for it by name:

```
$ strata run examples/book/ch11-prov.strata
0.9 :: controls(acme, shell)
  ⇐ [0.9 :: owns(acme, shell)]
0.804 :: controls(acme, target)
  ⇐ [0.9 :: owns(acme, shell)] ∧ [0.8 :: owns(shell, target)]
  ⇐ [0.3 :: owns(acme, target)]
0.8 :: controls(shell, target)
  ⇐ [0.8 :: owns(shell, target)]
0.9 :: owns(acme, shell)
0.3 :: owns(acme, target)
0.8 :: owns(shell, target)
```

Read what happened, because this listing is chapter 1's promise paid in full. `controls` is annotated `Prov` — full pedigree — and every derived fact prints the minimal sets of soft facts it rests on, one `⇐` line per proof, with the marginal in front computed by compiling exactly those proofs into a circuit that counts each fact once: `0.804` is the shared-evidence arithmetic of chapter 7, done by the pedigree itself. The auditor's derivation and the probability are one object now, printed together.

For most of this book's writing, that same declaration parsed and was then refused by name, with a stable code — `error[E0100]: the Prov/Prov_k annotation is not implemented` — never rejected as gibberish. That was the future-syntax frame, and the mechanism outlives its last tenant: the grammar of the full language shipped from day one, so a program written against it failed loudly and specifically, never silently and confusingly — the same courtesy the language extends to typos, extended to its own roadmap. The frame is empty now; everything the shipped grammar parses, it executes. The contract stands for whatever a later revision stages. And the fences that are *semantic* rather than staged hold exactly as drawn: exact provenance through recursion is impossible — a recursive soft fact has infinitely many derivation trees — so the checker still refuses a recursive `Prov` by name (`E1008`) and offers `Prov_k`, chapter 7's escape valve, whose printed answer says what it is: `(lower bound, top-k)`, a bound that only tightens.

## The phases, and the kind of promise each one is

The road from the repository you can clone to the system of chapters 10–11, in the order the engineering dependencies dictate — with each phase labeled by promise-kind: **[E]** settled engineering (the field knows how; it's work), **[D]** designed here, unbuilt (the kind chapter 10 *was*, until its engine was built against the waiting oracle), **[R]** open research (no one knows; declared as such).

1. **Scale the pedigree machinery.** Circuit compilation, exact weighted counting with gradients, top-k pedigrees, and a compilation cache all run at reference scale — demonstrated by the canonical neuro-symbolic exercise of learning digits from sums alone — and are now wired through the surface language: `Prov` and `Prov_k` are annotations you write, pedigrees you read, marginals you query — and an external compiler from the knowledge-compilation field's standard toolbox is now wired in as a differential oracle: the Python bridge hands a real SDD package the same proof DNFs, and the two counts must agree. Adopting one as the *scale* path remains **[E — the toolbox is known; it's work]**.
2. **Incrementality.** New facts arrive, conclusions update without recomputation — the delete-and-rederive reference runs, checked against full recomputation; the GPU version and the fuller differential machinery of chapter 5's line three remain **[E in the literature, D in this design]**. For the trading house this is the difference between a nightly batch and a compliance engine that answers *during* the trade.
3. **The GPU engine.** Chapter 10, executed — literally: columnar device-resident fixpoints, worst-case-optimal joins, the hybrid planner, the grounding pass, each bit-for-bit against the oracle **[built; the remaining race is performance hardening, decided by measurements in the repository]**.
4. **The neural boundary.** Predicates from models, gradients back through pedigrees — the interface runs today: `neural` facts flow into mode B, `?grad` sends the query's gradient back toward the named model, and the *in-process model* is now a library boundary too — a model object's forward pass supplies the soft facts at evaluation time. The boundary crosses languages: `import strata_k`, attach a Python callable, and a network trains *through* the logic layer — the training signal is logical ("this must be reachable, that must not"), and the engine's exact gradient arrives in the host framework as one more differentiable function **[built at reference scale; *production scale* honestly sits between E and R]**.
5. **Structured values** — `@terms` runs: constructor terms (lists, trees) via hash-consing, the language turning Turing-complete and trading away guaranteed termination, with a depth bound as the fence and a *sound-but-incomplete* status when it bites **[built at reference scale; the fence is the design]**.
6. **The bridges that don't exist yet [R, plainly]:** probability across the `@asp` fence — the semantics of "likelihood over self-consistent worlds" — is an open research area, and this book has already told you (chapter 8) that the type checker refuses the combination rather than improvise it. If the field settles it, the fence gets a gate. Until then, the constitution holds: no arithmetic without a semantics.

What is deliberately *not* on the road: nothing from chapter 5's refusals has crept back. No theorem proving, no continuous-domain constraint solving, no training framework, no general-purpose ambitions. The road makes the language more of what it is, not more things.

## Where this leaves you

The prologue asked you to carry one thing through eleven chapters: your own version of the sanctions smear — the rule in your codebase that is everywhere and nowhere. Here is the whole journey, measured against it.

The rule can be *written as what it is* — chapter 1's bargain, chapter 6's thirty lines, readable by the officer who owns it. It can carry cost and likelihood without lying about either — chapter 7's one word, and the line it refused to cross. It can face genuine choice — chapter 8's fenced worlds. It can be drafted by a machine and *caught* when the draft is wrong — chapter 9's one-token repair, the loop that never asked the model to be trustworthy. It can run at the speed of the hardware your industry already owns — chapter 10's bet, honestly boxed and since made good against its oracle. And at every step, the answer can show its work: the pedigree, the derivation an auditor can hold against the regulation, clause by clause. Programs that know why.

The third bet is placed in the open — the spec, the reference implementation, every listing in this book, and the differential harness that keeps the fast future honest all live in one repository, and the phases above are its issue tracker, not a prophecy. Clone it. Run the trading house. Break the checker; read its errors; feed a mandate of your own to a model and see what survives `strata check`. FGCS teaches that this paradigm's bets die of arriving before their hardware; chapter 2 argued the hardware has finally arrived, chapters 6 through 9 put the language in your hands, and the only piece of the argument no book can supply is the working engineer who tries the fifty-year-old idea on this decade's problem and decides, on the evidence, whether it holds.

That's you, and the repository is open. The rules, at last, can be the program.