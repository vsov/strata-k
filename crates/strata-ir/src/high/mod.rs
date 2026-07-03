//! High-IR: the public, LLM-writable source of truth. [IR-2..IR-5, IR-7, D2, D4]
//!
//! A desugared, typed program AST that serializes to the published JSON Schema.
//! Represents the WHOLE language (neural / @terms / @asp present structurally,
//! D5) so the v1.0 schema is stable even though Phase 0 executes only the
//! Bool/Trop fragment.

pub mod program;
pub mod sig;
pub mod trivia;

pub use program::{Atom, Fact, Item, ItemKind, Literal, Program, Rule, Term};
pub use sig::{Annotation, Effects, Signature};
pub use trivia::Trivia;
