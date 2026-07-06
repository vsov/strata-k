//! Core-IR: post-normalization internal vocabulary. [IR-6, D4]
//!
//! Fully desugared and stratified; the load-bearing interface both the reference
//! interpreter (`strata-eval`) and the future `strata-gpu` consume, and against
//! which differential tests compare backends. No trivia (that is High-IR only).
//! Constants are [`SymbolId`] (interned via [`crate::dict::SymbolDict`]), never
//! strings. Variables are canonical slot indices within their rule. The semiring
//! per predicate is resolved to `Bool` or `Trop` for Phase 0 (D5).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::dict::SymbolId;
use crate::high::program::AggOp;

/// The resolved semiring a predicate is evaluated under in Phase 0. [D5, D6]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Semiring {
    Bool,
    Trop,
}

/// A fully-normalized, stratified program. Rules and predicates carry their
/// stratum; the interpreter saturates stratum `k` before starting `k+1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CoreProgram {
    pub predicates: Vec<CorePred>,
    pub rules: Vec<CoreRule>,
    /// Number of strata; valid stratum numbers are `0..num_strata`.
    pub num_strata: u32,
}

impl CoreProgram {
    /// Rules of a given stratum, in declaration order.
    pub fn rules_in_stratum(&self, stratum: u32) -> impl Iterator<Item = &CoreRule> {
        self.rules.iter().filter(move |r| r.stratum == stratum)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CorePred {
    pub name: String,
    pub arity: u32,
    pub semiring: Semiring,
    pub stratum: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CoreRule {
    pub head: CoreAtom,
    pub body: Vec<CoreLiteral>,
    pub stratum: u32,
    /// Number of distinct variable slots used across head+body.
    pub var_count: u32,
    /// Obligation threaded from CHECK-14: this (Trop) rule may carry negative
    /// weights, so the evaluator MUST run negative-cycle detection (EVAL-7).
    /// The checker sets it; the evaluator honors it — closing the check↔eval seam.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub neg_weight_cycle_check: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CoreAtom {
    pub pred: String,
    pub args: Vec<CoreTerm>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum CoreLiteral {
    Pos(CoreAtom),
    /// Negated literal — resolved against a strictly lower stratum (spec 1.2).
    Neg(CoreAtom),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum CoreTerm {
    /// Canonical variable slot within the rule (`0..var_count`).
    Var { slot: u32 },
    /// Interned constant.
    Const { sym: SymbolId },
    /// Integer literal / tropical weight source.
    Int { value: i64 },
    /// Aggregate over a variable slot (spec 1.3).
    Agg { op: AggOp, slot: u32 },
    /// Constructor term `functor(args...)` (`@terms`, spec §1.4); args are
    /// themselves terms (slots, constants, or nested compounds).
    Compound {
        functor: SymbolId,
        args: Vec<CoreTerm>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_partition_by_stratum() {
        let prog = CoreProgram {
            predicates: vec![CorePred {
                name: "path".into(),
                arity: 2,
                semiring: Semiring::Bool,
                stratum: 0,
            }],
            rules: vec![
                CoreRule {
                    head: CoreAtom {
                        pred: "path".into(),
                        args: vec![],
                    },
                    body: vec![],
                    stratum: 0,
                    var_count: 0,
                    neg_weight_cycle_check: false,
                },
                CoreRule {
                    head: CoreAtom {
                        pred: "q".into(),
                        args: vec![],
                    },
                    body: vec![],
                    stratum: 1,
                    var_count: 0,
                    neg_weight_cycle_check: false,
                },
            ],
            num_strata: 2,
        };
        assert_eq!(prog.rules_in_stratum(0).count(), 1);
        assert_eq!(prog.rules_in_stratum(1).count(), 1);
    }
}
