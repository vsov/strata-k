//! Check diagnostics: the E1xxx code block. [CHECK-1, D9]
//!
//! Re-exports the shared [`Diagnostics`] collector (IR-9); strata-check owns the
//! E1xxx code range. (High-IR nodes do not yet carry source spans, so check
//! diagnostics currently use a zero span — the message names the offending
//! predicate/rule. Attaching spans is a follow-up on IR-4.)

pub use strata_ir::diag::Diagnostics;

pub mod codes {
    use strata_ir::diag::DiagCode;

    /// A predicate is used but never declared (CHECK-2, D3).
    pub const UNDECLARED_PRED: DiagCode = DiagCode(1001);
    /// Negation/aggregation through a cycle: not stratifiable (CHECK-3, spec 1.2).
    pub const UNSTRATIFIABLE: DiagCode = DiagCode(1002);
    /// A head or negated-literal variable is not bound by a positive body
    /// literal (range-restriction / safety, CHECK-13).
    pub const NOT_RANGE_RESTRICTED: DiagCode = DiagCode(1003);
    /// A fact contains a non-ground term.
    pub const NON_GROUND_FACT: DiagCode = DiagCode(1004);
    /// An atom's arity does not match its declaration.
    pub const ARITY_MISMATCH: DiagCode = DiagCode(1005);
    /// A predicate's annotation is not executable in Phase 0 (Prov/Prov_k).
    pub const NOT_EXECUTABLE: DiagCode = DiagCode(1006);
    /// A rule mixes incompatible semirings (e.g. Trop body into a Bool head).
    pub const SEMIRING_CONFLICT: DiagCode = DiagCode(1007);
    /// A forbidden cell of the semiring×recursion table 2.4 (spec 2.4): e.g.
    /// exact probabilistic provenance through recursion. Carries the nearest
    /// allowed alternative in its message (D9/I4).
    pub const TABLE_2_4_FORBIDDEN: DiagCode = DiagCode(1008);

    pub const ALL: &[(DiagCode, &str)] = &[
        (UNDECLARED_PRED, "check.undeclared-predicate"),
        (UNSTRATIFIABLE, "check.unstratifiable"),
        (NOT_RANGE_RESTRICTED, "check.not-range-restricted"),
        (NON_GROUND_FACT, "check.non-ground-fact"),
        (ARITY_MISMATCH, "check.arity-mismatch"),
        (NOT_EXECUTABLE, "check.not-executable-annotation"),
        (SEMIRING_CONFLICT, "check.semiring-conflict"),
        (TABLE_2_4_FORBIDDEN, "check.table-2.4-forbidden"),
    ];
}

#[cfg(test)]
mod tests {
    use super::codes;

    #[test]
    fn codes_are_unique_and_in_e1_range() {
        let mut seen = std::collections::HashSet::new();
        for (c, _) in codes::ALL {
            assert!(seen.insert(c.0), "duplicate code {c}");
            assert!(
                (1000..2000).contains(&c.0),
                "check code {c} out of E1xxx range"
            );
        }
    }
}
