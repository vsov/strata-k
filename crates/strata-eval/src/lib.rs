//! `strata-eval` — the reference interpreter over Core-IR: the eternal
//! differential oracle (I5). Ships the naive `T_P` fixpoint (obviously correct);
//! a semi-naive delta mode cross-checked against it is EVAL-8 (Slice 12), D7.
//!
//! Consumes Core-IR (IR-6) directly, so it executes a hand-written Core-IR value
//! before any parser or checker exists (D15, Slice 2 — the tracer). Bool + Trop
//! (режим A) in Phase 0 (D5, D6).

pub mod naive;
pub mod prob;
pub mod seminaive;
pub mod store;
pub mod value;

pub use naive::{run, EvalError};
pub use prob::{marginals, ProbError};
pub use seminaive::run_semi_naive;
pub use store::{Db, Relation, Tuple};
pub use value::{Ann, GroundVal};
