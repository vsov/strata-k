//! Front-end diagnostics: the E0xxx code block. [FRONT-3, D9]
//!
//! The [`Diagnostics`] collector and renderers live in strata-ir (IR-9); front
//! re-exports them and owns the E0xxx code range. INFRA-10 checks the compiler
//! never emits a code absent from [`codes::ALL`].

pub use strata_ir::diag::Diagnostics;

/// The E0xxx registry owned by strata-front. Each code is allocated once.
pub mod codes {
    use strata_ir::diag::DiagCode;

    /// Unexpected character(s) during lexing.
    pub const LEX_UNEXPECTED: DiagCode = DiagCode(1);
    /// Expected one token/construct, found another.
    pub const PARSE_EXPECTED: DiagCode = DiagCode(2);
    /// Input ended while a construct was still open.
    pub const PARSE_UNEXPECTED_EOF: DiagCode = DiagCode(3);
    /// A variable occurs exactly once in a rule (D3: error, not warning).
    pub const SINGLETON_VAR: DiagCode = DiagCode(10);
    /// A construct is grammatical but not executable in Phase 0 (D5).
    pub const NOT_IMPLEMENTED: DiagCode = DiagCode(100);
    // Undeclared-predicate detection (E1xxx) is owned by strata-check (CHECK-2).

    /// Every code above, for registry-conformance checks (INFRA-10).
    pub const ALL: &[(DiagCode, &str)] = &[
        (LEX_UNEXPECTED, "lex.unexpected-character"),
        (PARSE_EXPECTED, "parse.expected"),
        (PARSE_UNEXPECTED_EOF, "parse.unexpected-eof"),
        (SINGLETON_VAR, "wf.singleton-variable"),
        (NOT_IMPLEMENTED, "phase0.not-implemented"),
    ];
}

#[cfg(test)]
mod tests {
    use super::codes;

    #[test]
    fn codes_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for (c, _) in codes::ALL {
            assert!(seen.insert(c.0), "duplicate code {c}");
        }
    }
}
