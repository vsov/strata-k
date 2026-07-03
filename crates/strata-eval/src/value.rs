//! Semiring annotations for the reference interpreter. [EVAL-2]
//!
//! The ground value type [`GroundVal`] lives in strata-ir (shared with the
//! checker) and is re-exported here for the interpreter's internal paths.

use strata_ir::core::Semiring;
use strata_ir::trop::{TropOverflow, Weight};

pub use strata_ir::value::GroundVal;

/// A semiring annotation attached to a derived tuple.
///
/// `Unit` is the Bool annotation (a tuple is present-or-not); `W` is the
/// tropical weight. One uniform annotation lets a single fixpoint driver serve
/// both semirings (D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ann {
    Unit,
    W(Weight),
}

impl Ann {
    /// An EDB annotation from an optional weight: `None` → Bool `Unit`,
    /// `Some(w)` → tropical `W(w)`. Used to seed the EDB from lowered facts.
    pub fn from_weight(weight: Option<Weight>) -> Ann {
        match weight {
            None => Ann::Unit,
            Some(w) => Ann::W(w),
        }
    }

    /// `⊗` identity for `sem` (neutral for the product): Bool `Unit`, Trop `0`.
    pub fn otimes_id(sem: Semiring) -> Ann {
        match sem {
            Semiring::Bool => Ann::Unit,
            Semiring::Trop => Ann::W(Weight::OTIMES_ID),
        }
    }

    /// `⊕` — combine two derivations of the *same* tuple. Bool: OR (idempotent
    /// `Unit`); Trop: `min`. Returns the combined value and whether it changed
    /// `self` (used to detect fixpoint progress).
    pub fn oplus(self, other: Ann) -> (Ann, bool) {
        match (self, other) {
            (Ann::Unit, Ann::Unit) => (Ann::Unit, false),
            (Ann::W(a), Ann::W(b)) => {
                let m = a.oplus(b);
                (Ann::W(m), m < a)
            }
            _ => panic!("semiring annotation mismatch in oplus: {self:?} vs {other:?}"),
        }
    }

    /// `⊗` — combine annotations along one derivation (product of body literals).
    /// Bool: AND (`Unit`); Trop: checked `+` (overflow is an error, D6).
    pub fn otimes(self, other: Ann) -> Result<Ann, TropOverflow> {
        match (self, other) {
            (Ann::Unit, Ann::Unit) => Ok(Ann::Unit),
            (Ann::W(a), Ann::W(b)) => a.otimes(b).map(Ann::W),
            _ => panic!("semiring annotation mismatch in otimes: {self:?} vs {other:?}"),
        }
    }
}
