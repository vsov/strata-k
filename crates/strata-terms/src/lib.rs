//! Structural terms and their program machinery. [spec Phase 3 — термы]
//!
//! Datalog gains function symbols (lists, trees, records) here. Recursive rules
//! that *build* terms need three classic guards to stay finite and efficient,
//! and a demand transformation to stay goal-directed:
//!
//! - [`intern`] — **host interning**: hash-cons ground terms to integer ids
//!   (structure sharing), with a **depth bound** so term-building recursion
//!   terminates.
//! - [`subsume`] — **subsumption**: a more general atom subsumes a more specific
//!   one; used to keep only maximally-general facts.
//! - [`magic`] — the **magic-sets** transformation: rewrite a program + adorned
//!   query so bottom-up evaluation computes only the demand-relevant facts.
//! - [`pointsto`] — the exit: an Andersen points-to analysis that exercises all
//!   of the above and reports the fraction of time spent interning.
//!
//! Device-side interning (moving the hash-cons to the GPU by profile) is the
//! documented follow-up; everything here is the host stage.

pub mod intern;
pub mod magic;
pub mod pointsto;
pub mod subsume;

pub use intern::{Interner, TermId};
pub use subsume::{subsumes, Term};
