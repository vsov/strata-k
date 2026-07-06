# Prologue: Three Attempts

Here is a sentence from the world I work in:

> A trade must not clear if any beneficial owner of the counterparty, at any depth, appears on a sanctions list.

A regulator can write that sentence. A compliance officer can read it. A new hire understands it on day one. Now try to find it in the codebase.

In every trading system I have seen — including the ones I built — that sentence does not live anywhere. It is smeared. There is a recursive ownership walk in the onboarding service. There is a nightly job that flattens the ownership graph into a cache, because the walk was too slow to run per-trade. There is a depth limit of three in the risk engine, added by someone who feared cycles and never removed. There is a hotfix from two years ago that special-cases one counterparty, with a comment that says `// see JIRA-4711`, and JIRA-4711 was deleted in a migration. Ask "where do we enforce the sanctions rule?" and the honest answer is: everywhere, and therefore nowhere.

This is not a story about bad engineers. The engineers were good. It is a story about writing *rules* in languages built for *procedures*. Java, C++, and their relatives are magnificent at describing how: fetch this, loop over that, update the other. The sanctions sentence contains no how at all. It is pure what — a statement about which facts must hold. Translating what into how is a lossy compilation step, and we perform it by hand, under deadline, and then we maintain the compiled output forever while the source — the sentence — lives only in a PDF and in shared tribal memory.

I have come to believe that every trading system is a logic program, implemented badly, by hand, by people who were never told that's what they were doing. Compliance is rules. Risk limits are rules. Order routing is rules. Margin, netting, eligibility, settlement — rules, rules, rules. We encode them as control flow, and the control flow slowly eats them.

Before settling on that diagnosis, I tried two others — each a real project, each built to test a theory of what was missing.

## Attempt one: the engine is the problem

The first theory was that the *machinery* was missing. Rules engines existed — CLIPS, the classic expert-system shell, has been matching rules against facts since the 1980s — but nothing in that lineage could live inside a trading system, where the budget per event is measured in microseconds. So I rebuilt one. The project, reclips, was a clean-slate reengineering of the CLIPS shell: same rule-matching idea, new body — columnar fact storage, a delta-based matcher, preallocated memory, deterministic replay, latency you could put on a hot path.

The engine worked. And working, it taught me what the actual problem was. When you feed a classic rule engine a set of rules, the answer you get can depend on the *conflict-resolution strategy* — the policy that decides which rule fires first when several match. CLIPS ships seven of them. Seven. Think about what that means: the rules alone do not determine the answer. The rules plus a scheduling policy determine the answer. Your knowledge doesn't mean anything by itself; it means something only in the company of an execution order. I had built a very fast machine for evaluating programs that had no fixed meaning.

## Attempt two: the hardware is the problem

So I went the other way. If I couldn't fix what programs *mean*, I could at least fix what they *cost*. The next project, an ahead-of-time compiler for reactive dataflow programs, knew things about hardware that most programmers politely ignore: cache-line geometry, NUMA placement, huge pages, the price of a branch. It compiled event-processing pipelines down to code that respected physics.

It worked too. The systems got faster. They did not get one bit clearer. The sanctions sentence was still smeared across services — the smear now executed in fewer nanoseconds. Fast confusion is still confusion, and speed had never been why the rule took a hotfix and a dead JIRA ticket to enforce.

Neither project was wasted — an engine and a compiler are correct answers to the questions they ask, and both still earn their keep. But neither question was *this* question. The missing piece was a *language*: one where the regulator's sentence is the program — not the inspiration for the program, the program — where the order you write things in cannot change what they mean, and where every answer can show its work, because in a regulated industry an answer without a pedigree is a liability.

## The old idea

Here is the uncomfortable part: that language already exists, and it is fifty years old. Logic programming — writing programs as facts and rules and letting an engine derive the consequences — carries a history of spectacular promise and two spectacular collapses, including the largest state-funded bet on a programming paradigm ever made. Chapter 2 tells that story properly, because if I'm asking you to take this idea seriously in 2026, I owe you an honest account of why it failed in 1992.

But two things have changed since then, and they change everything.

The first is hardware. The old logic languages were built around a sequential search procedure that fights modern silicon at every step. The branch of the family this book is about — Datalog — computes by a different scheme, one that is embarrassingly parallel by nature. For decades that was a curiosity. Then the machine-learning boom filled the world with GPUs. The hardware the paradigm always needed got built, at planetary scale, for someone else's reasons.

The second is language models. LLMs write code now — yours and mine — and they write it fluently, confidently, and sometimes wrongly, and the bottleneck of software has quietly moved from *writing* to *verifying*. That inverts what we should optimize a programming language for. For fifty years we optimized languages for the writer. A language optimized for the *checker* looks different: small, declarative, order-independent, with answers that carry their own derivations. It looks, in fact, like a logic language — which suggests these fifty-year-old ideas were not wrong, just early.

This book stands on that inversion, and its argument fits in one sentence: **let the language model propose the rules, let a symbolic core guarantee what follows from them, and let the GPU make it fast.**

## The third attempt

The second half of this book introduces Strata-K, a logic language I am building in the open — the third attempt, the one aimed at the actual missing piece.

I want to be precise with you about its status, here on the first pages, because the rest of the book depends on your trust. Today, Strata-K is a working reference implementation: the language core runs end-to-end on a CPU — parsing, checking, evaluation, exact probabilistic queries and their gradients, full pedigrees with their marginals, structural terms, an answer-set solver — and is cross-checked against an established independent engine and fuzzed against itself. The GPU engine, designed in detail first, has since been built and is validated bit-for-bit against that reference. Where this book describes what runs, you can run it: every code listing executes today, and every output shown is the real output. Where the book describes design, it says so in the open. And this book carries no benchmark numbers: measurements live in the repository, next to the oracle that keeps them honest, where they can age without quietly falsifying a printed page.

What I ask of you is narrower than belief. You write C++, or Java, or something in their family, and somewhere in your current codebase sits your own version of the sanctions smear — the rule that is everywhere and nowhere. Hold that rule in mind for eleven chapters. The third attempt's bet is that by the end you will want to write it down as what it is.
