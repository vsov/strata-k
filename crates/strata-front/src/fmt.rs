//! `strata fmt` core: parse → canonical print, zero options. [FRONT-9, D12]
//!
//! Formatting rests entirely on the parser and printer and is idempotent by
//! construction. Errors block formatting (there is nothing canonical to emit),
//! so callers get the diagnostics instead.

use crate::diagnostics::Diagnostics;
use crate::parser::parse;
use crate::printer::print_program;

/// Format `src` to canonical surface, or return the diagnostics that blocked it.
pub fn format(src: &str) -> Result<String, Diagnostics> {
    let (program, diags) = parse(src);
    if diags.has_errors() {
        Err(diags)
    } else {
        Ok(print_program(&program))
    }
}

/// Is `src` already in canonical form? (`fmt --check`.)
pub fn is_formatted(src: &str) -> bool {
    matches!(format(src), Ok(canon) if canon == src)
}
