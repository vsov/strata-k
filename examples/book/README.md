# Book listings

Every listing referenced by the book (`book/en/`). CI contract: output printed in
the book equals real CLI output from these files.

Expected to **run clean** (`strata run`, exit 0):
`ch01-ownership`, `ch06-trading-house`, `ch07-routes`, `ch07-shared-evidence`,
`ch08-portfolio`, `ch09-vignette`.

Expected to **fail on purpose** (the failure *is* the listing):
- `ch09-vignette-draft.strata` — `strata check` → `E1001` (the chapter-9 typo demo); exit 1.
- `ch11-neural.strata` — `strata check` → `E0100` "not implemented in Phase 0" (future-syntax demo); exit 1.

Expected to **run clean but be semantically wrong** (the wrongness is the listing):
- `ch09-vignette-draft2.strata` — swapped `owner_within2` arguments; `check` passes, `run`
  yields `cleared(dunlin)` where the mandate requires `blocked(dunlin)`. The chapter-9
  demonstration of the verification layer *below* the type checker.

Measured third-party counterexamples for chapter 4 live in [`ch04/`](ch04/README.md).
