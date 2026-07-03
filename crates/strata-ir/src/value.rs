//! Ground values and EDB facts — shared by the checker (which lowers to them)
//! and the interpreter (which executes them). Lives in strata-ir so neither
//! sibling depends on the other (D14).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::dict::SymbolId;
use crate::trop::Weight;

/// A ground argument value: an interned constant or an integer literal.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum GroundVal {
    Sym(SymbolId),
    Int(i64),
}

/// A ground EDB fact produced by lowering (CHECK-10) and consumed by eval.
/// `weight = None` is a Bool fact; `Some(w)` is a tropical fact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GroundFact {
    pub pred: String,
    pub args: Vec<GroundVal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<Weight>,
}
