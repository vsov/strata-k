# 12 · First Contact

Chapter 11 ended with an invitation: clone the repository, run the trading
house, break the checker. This chapter is about what happened when someone
did.

Not a user, yet — a reviewer. Several rounds of one, in fact: an outside
examiner who checked out the tree, ran every gate, and wrote back verdicts
that all orbited a single sentence: *this is a serious research system with
real vertical scenarios, and what it lacks is not features but the opposite —
narrowing, stabilization of the public core, and a hard separation of the
proven from the experimental.*

Every previous chapter of this book added something. This one is about
subtraction — and about a class of defect the book hasn't named yet, the kind
that only appears when a second pair of hands touches the system. The
engine's answers were already checked against oracles; chapter 9's checker
already caught the wrong *program*. What nobody had tested was the wrong
*call* — the first external user writing perfectly reasonable, slightly
imperfect code against the public surface, and the system's obligation to
fail loudly instead of answering wrongly.

## Workloads: claims become checked facts

The first addition of the round was, admittedly, an addition — but of a
particular kind. The repository's examples were syntax demonstrations: five
facts, two rules, one query. Between them and the book's trading house there
was nothing you could point a skeptic at — no program with *data*.

So the repository grew a workload tier: an anti-money-laundering ownership
screen — sixty firms, a corporate registry of certain facts, ten model flags
with confidences, transitive control, an investigation set with `Prov_k`
pedigrees, and clearance by negation *over the soft-derived evidence* — and,
beside it, all-pairs cheapest cost over the tropical semiring on a grid with
express links: chapter 7's "change one word," at data scale.

The interesting decision was not the programs; it was the contract around
them. Every number their READMEs quote is pinned in CI — the test runs the
real binary and asserts the exact output lines, and a new workload directory
*fails the suite* until its output is pinned too. The data files ship with
the deterministic generator that produced them, and CI re-runs each generator
and compares the file sets **both ways**: every generated file must
byte-equal its committed twin, and no committed data file may survive that
the generator no longer produces.

Why this ceremony? Because prose rots and tests don't. A README that says
"the marginal is 0.98" is a claim; a test that fails the build when the
marginal stops being 0.98 is a fact. The two-way file comparison exists
because the reviewer caught the one-way version overpromising: "committed
data == generator output" is a statement about *sets*, and checking only one
inclusion lets a stale file live forever inside a green build. The general
lesson became a house rule: when documentation asserts an equality, CI must
check both directions of it.

## The query that did nothing

The same workload work surfaced an embarrassment. The grammar had always
allowed a plain query — `?route(hub, _)` — and the checker dutifully parsed
and validated it. And then a plain `strata run` ignored it entirely and
printed the whole database. The query was a no-op: syntax that looked like it
did something, checked as if it did something, and did nothing.

For a language whose founding bargain is *say what you mean and the system
answers exactly that*, a decorative query is a small betrayal. There were two
honest exits: document it as an introspection declaration — "this line is
metadata" — or make it mean the obvious thing. The obvious thing won. A plain
`?q(a, _)` now filters the run's output: only queried predicates print, and
only the tuples whose ground positions match; with no query the whole
database prints, as before. The routing workload dropped from a
four-thousand-line dump to the three lines its README actually discusses —
which is what its author meant all along.

The design detail worth keeping: the fix went in only after checking that no
pinned output anywhere — book listings, workloads — relied on the old
behavior. Making a no-op meaningful is the safest kind of language change
precisely when you can prove nobody depended on the nothing.

## Drawing the stability line

The reviewer's sharpest structural point was about promises, not code. By
this point the tree held a deductive core with oracles, a probabilistic layer
with three independent cross-checks, an ASP island, structural terms, an
incremental-provenance maintainer, a GPU engine, and a Python bridge — and
the documentation presented them with roughly equal confidence. That is its
own kind of dishonesty. A reader cannot tell the load-bearing walls from the
scaffolding.

So the language now carries an explicit stability contract. The **stable
kernel** — the CLI, the library facade and the Python bridge, `Bool`/`Trop`,
the probabilistic layer with `?prob`/`?grad` and `Prov`/`Prov_k`, structural
terms with their depth-bound status line, the loaders, the query filters, the
worked examples — is the part whose behavior and API the project intends to
keep, and each entry earns its place the same way: an oracle or a pinned CI
check stands behind it. Everything else — the `@asp` island, incremental
provenance, the GPU engine — is marked **experimental**, in the reference
manual and again at the top of each crate, with the reason stated rather than
implied: the surface is young, the API may move, depend on it knowingly.

Two lessons came out of merely *writing* the contract. First: the initial
draft of the README disagreed with it — one public sentence put the ASP layer
inside the stable surface while the reference manual put it outside, and a
reviewer caught the drift within a day. Status statements are the easiest
documentation to contradict yourself in, because they live in many files and
decay independently; now any edit to one goes hunting for the others. Second:
the temptation to widen the stable set is constant — everything *feels*
solid from the inside. The discipline is to promise only what an oracle
already enforces, so that the contract is a report, not an aspiration.

## The first imperfect caller

Then came the misuse audit — the reviewer's most valuable round, and the
reason this chapter exists. The premise: stop reviewing whether the engine is
right, and start reviewing what happens to the first person who holds it
slightly wrong. Attach the neural models twice. Load the input files twice.
Load them once, have the second file missing, fix it, retry. Feed the CLI a
misspelled flag, two files, an enum value it never heard of. Hand the CSV
loader the output of a corrupted export.

The engine passed none of these gracefully at first contact, and the failures
shared a signature worth naming. None of them crashed. They *answered* — with
the wrong number. Attach the same model twice and every soft fact doubled: a
marginal of 0.9 became 0.99, no error, no warning. Load inputs twice, same
arithmetic. Fail halfway through a load and retry, and the rows read before
the failure were in the database twice. A misspelled `--semi-naive` was
silently ignored, so you benchmarked the engine you didn't ask for. A mangled
quoted field — `"acme"junk` — loaded as the constant `acmejunk` and joined
with nothing, or worse, with something.

This is the worst class of defect a probabilistic system can have, and it is
worth spelling out why. Chapter 9 argued that the checker's virtue is turning
a model's plausible-but-wrong draft into a *loud, specific* error. Silent
duplication is the exact inversion: a plausible-and-right call producing a
quietly wrong probability. No oracle catches it — the engine is correctly
counting the wrong database. The trading house's compliance officer doesn't
see an exception; she sees 0.99 where the evidence says 0.9, and she acts on
it.

The fixes are small and their principles are general. Every method that
appends to program state is now **once-only** — a second attach or load is a
typed error naming the misuse, never a silent shift — and **transactional**:
the loader validates every file and every row into local buffers and commits
only on full success, so a failure partway through leaves nothing behind and
a retry is clean, not doubled. (The once-guard alone was not enough — the
reviewer proved it by failing a load *in the middle*, where the guard hadn't
armed yet but half the rows were already in. Both properties have to hold,
and both now have tests that hold them: repeat-after-success refuses;
fail-then-retry yields exactly the facts on disk.) The CLI grew a strict
argument layer — unknown flags, extra files, and invalid enum values are
usage errors, and `--` exists for the filename that legitimately starts with
a dash. The CSV splitter now insists a quoted field be *whole*: after the
closing quote, only a delimiter or the end of the line — a corrupted export
fails with a filename and line number instead of becoming data.

And the guards live in the engine, not the wrapper. The Python bridge had
grown its own second-attach refusal a round earlier; the reviewer's follow-up
pointed at the Rust facade underneath, which still duplicated happily for
anyone who embedded it directly. A guard in the convenience layer is a
courtesy; a guard in the public core is a contract. It moved down.

## Iron, weighed honestly

One more addition from the same season needs its caveat told, because the
caveat *is* the content. The GPU engine of chapter 10 finally raced the
reference interpreter on the same closure problem — both engines, same
program, and the harness refuses to print a ratio unless both sides' answers
match exactly, so a speedup can never be quoted off a wrong result. The
device won by orders of magnitude, and the repository deliberately does not
lead with the number.

Here is why. The reference interpreter is the *obviously correct* oracle —
its join is a naive nested loop, because chapter 6 chose legibility over
speed for the thing every other engine is measured against. Racing it and
quoting the multiplier would be arithmetic against a straw man; an optimized
CPU Datalog would close most of that gap. The honest claim, and the one the
documentation makes, is narrower and more useful: mode A at data scale needs
a device backend to stay interactive, and the backend exists, bit-exact
against its oracle, measured on one named machine. Performance claims scoped
any wider than their evidence are just chapter 4's autopsy subjects, waiting.

## What subtraction bought

Count what this chapter actually added to the language: one output filter.
Everything else was narrowing — claims pinned to tests, a stability line
drawn and defended against its own documentation, misuse turned from silent
corruption into typed refusal, a benchmark reframed from trophy to tool. The
reviewer's arc across the rounds bent accordingly: from *research workbench,
not a product* toward *practically applicable in a narrow, valuable niche* —
with the standing warning that the enemy now is not missing capability but
swelling promises.

That warning deserves the last word, because it is chapter 5's lesson wearing
work clothes. The refusals that defined this language — no arithmetic without
a semantics, no probability across the ASP fence, no general-purpose
ambitions — were subtractions too, and they are why the checker can promise
what it promises. First contact taught the same discipline at a smaller
scale: every public method is a promise, every README sentence is a promise,
and a promise you cannot pin to a failing test is a liability with good
manners. The road ahead still runs through chapter 11's phases. But it is
paved, from here on, one checked fact at a time.
