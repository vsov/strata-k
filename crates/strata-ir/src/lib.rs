//! `strata-ir` — the IR data model and the three cross-cutting contracts every
//! other crate depends on. Base of the workspace: no dependencies on sibling
//! crates (D14, refined in phase0-plan.md §1).
//!
//! Module map (owning task in brackets):
//! - [`version`]  ir_version + compatibility policy              [IR-1]
//! - [`trop`]     Trop weight scalar: i64 + distinct +∞ + overflow [IR-3]
//! - [`high`]     High-IR: public, LLM-writable, source of truth   [IR-2..IR-5]
//! - [`core`]     Core-IR: internal, fed to interpreter + GPU      [IR-6]
//! - [`schema`]   schemars JSON Schema gen + version-gated load    [IR-7]
//! - [`dict`]     symbol dictionary (constant → scalar id)         [IR-8]
//! - [`diag`]     shared Diagnostic + unified DiagCode registry    [IR-9]
//! - [`output`]   canonical output-format contract                [IR-10]
//!
//! JSON encoding convention: see docs/ir-encoding.md.

pub mod core;
pub mod diag;
pub mod dict;
pub mod high;
pub mod output;
pub mod schema;
pub mod terms;
pub mod trop;
pub mod value;
pub mod version;

pub use version::{is_compatible, Version, IR_VERSION, IR_VERSION_STR};
