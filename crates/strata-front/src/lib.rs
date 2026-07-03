//! `strata-front` — lexer, recursive-descent parser, canonical pretty-printer,
//! `strata fmt`, and the diagnostics engine (emitting the shared
//! [`strata_ir::diag::Diagnostic`], IR-9).
//!
//! Owns the E0xxx diagnostic-code block. Depends only on `strata-ir` (D14).
//! Grammar source of truth: docs/grammar.ebnf.

pub mod diagnostics;
pub mod fmt;
pub mod lexer;
pub mod parser;
pub mod printer;

pub use diagnostics::{codes, Diagnostics};
pub use fmt::{format, is_formatted};
pub use parser::parse;
pub use printer::print_program;
