//! Canonical output-format contract. [IR-10, D6, exit-item 1]
//!
//! The single definition of "canonical relation output" that the eval printer,
//! the CLI, and the Soufflé-output parser must all agree on — otherwise the
//! bit-for-bit diff (exit-item 1) fails with no single owner. Covers tuple sort
//! order, per-column rendering, the weight column, `+∞` rendering, and the order
//! relations are emitted in.

/// How a Trop `+∞` weight renders in canonical text/JSON output (D6). Chosen so
/// it can never be confused with a finite integer. Shared by [`crate::trop::Weight`].
pub const POS_INF_TOKEN: &str = "inf";

/// Render one already-resolved column value (constant/int) for canonical output.
/// Constants must already be resolved to their string via the symbol dictionary
/// (IR-8) so no dict-id leaks into output (the INFRA-3 `.dl` concern).
pub fn render_atom_value(s: &str) -> &str {
    s
}

// TODO(IR-10): `fn canonicalize(relations) -> String` producing the sorted,
// deterministic, Soufflé-diffable database rendering; the inverse parser reading
// Soufflé's output back; column/weight rendering rules. Consumed by EVAL-9,
// CLI-8, INFRA-3/4. Kept as the naming/constants anchor at Slice 1.

#[cfg(test)]
mod tests {
    #[test]
    fn inf_token_is_not_an_integer() {
        assert!(super::POS_INF_TOKEN.parse::<i64>().is_err());
    }
}
