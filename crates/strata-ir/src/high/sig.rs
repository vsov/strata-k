//! Predicate signatures: the three-component type from spec 1.7. [IR-2]
//!
//! Every `pred` declaration (D3) carries argument domains, an annotation type
//! (semiring/provenance), and effects. The full lattice (Bool/Trop/Prov/Prov_k)
//! and all effect axes are representable now, though Phase 0 executes only
//! Bool/Trop (D5), so the v1.0 schema is stable.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Argument type of one predicate position.
///
/// Adjacently tagged (docs/ir-encoding.md rule 2): `{"kind":"domain","data":{"name":"node"}}`.
/// The common `domain` case is also accepted as a bare string for LLM ergonomics
/// via [`ArgType`]'s custom handling? — no: kept strict/adjacent for uniformity;
/// the surface sugar lives in the parser, not the IR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ArgType {
    /// A named constant domain (`.type`-like).
    Domain { name: String },
    /// A tropical/numeric weight column, i64 (D6).
    Int,
    /// Structural term type for `@terms` — present but not executed in Phase 0 (D5).
    Term { name: String },
}

/// Annotation (semiring / provenance) type. [spec 1.7]
///
/// Pure-payload variants use adjacent tagging; the unit variants serialize as
/// `{"kind":"bool"}` etc. Coercion order: `Bool ⊑ Trop`, `Bool ⊑ Prov`,
/// `Prov ⊑ Prov_k`. **`Trop` and `Prov` are incomparable.**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Annotation {
    Bool,
    Trop,
    Prov,
    /// Top-k provenance approximation; mandatory under recursive soft deps.
    ProvK {
        k: u32,
    },
}

impl Annotation {
    /// Is `self` coercible **to** `target` in the 1.7 lattice? Reflexive.
    ///
    /// Edges: `Bool→Trop`, `Bool→Prov`, `Prov→Prov_k`. No `Trop↔Prov` either way
    /// (idempotent `min` is incompatible with probabilistic disjunction).
    pub fn is_coercible_to(&self, target: &Annotation) -> bool {
        use Annotation::*;
        match (self, target) {
            (Bool, _) => true,
            (a, b) if a.same_kind(b) => true,
            (Prov, ProvK { .. }) => true,
            _ => false,
        }
    }

    fn same_kind(&self, other: &Annotation) -> bool {
        use Annotation::*;
        matches!(
            (self, other),
            (Bool, Bool) | (Trop, Trop) | (Prov, Prov) | (ProvK { .. }, ProvK { .. })
        )
    }

    /// Least upper bound in the coercion order, or `None` if incomparable
    /// (the `Trop`⋈`Prov` case → CHECK-5 reports an explicit-conversion error).
    pub fn lub(&self, other: &Annotation) -> Option<Annotation> {
        if self.is_coercible_to(other) {
            Some(*other)
        } else if other.is_coercible_to(self) {
            Some(*self)
        } else {
            None
        }
    }

    /// Executable in Phase 0? Only Bool and Trop run (D5).
    pub fn is_phase0_executable(&self) -> bool {
        matches!(self, Annotation::Bool | Annotation::Trop)
    }
}

/// Termination effect. [spec 1.7]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Termination {
    #[default]
    Total,
    Partial,
}

/// Completeness effect: `sound_only` is set under an active depth bound. [spec 1.4]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Completeness {
    #[default]
    Complete,
    SoundOnly,
}

/// Determinism effect: `stochastic` when neural inference with nondeterminism.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Determinism {
    #[default]
    Deterministic,
    Stochastic,
}

/// The effect triple. Defaults are the strongest guarantees, so an omitted
/// `effects: {}` in JSON means `total`/`complete`/`deterministic`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Effects {
    pub termination: Termination,
    pub completeness: Completeness,
    pub determinism: Determinism,
}

/// The full three-component signature bound to a predicate. [spec 1.7]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Signature {
    pub args: Vec<ArgType>,
    pub annotation: Annotation,
    #[serde(default)]
    pub effects: Effects,
}

impl Signature {
    pub fn arity(&self) -> usize {
        self.args.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coercion_lattice_edges() {
        use Annotation::*;
        assert!(Bool.is_coercible_to(&Trop));
        assert!(Bool.is_coercible_to(&Prov));
        assert!(Prov.is_coercible_to(&ProvK { k: 3 }));
        // the two incomparable non-edges:
        assert!(!Trop.is_coercible_to(&Prov));
        assert!(!Prov.is_coercible_to(&Trop));
    }

    #[test]
    fn lub_bool_trop_is_trop() {
        assert_eq!(
            Annotation::Bool.lub(&Annotation::Trop),
            Some(Annotation::Trop)
        );
    }

    #[test]
    fn lub_trop_prov_is_none() {
        assert_eq!(Annotation::Trop.lub(&Annotation::Prov), None);
    }

    #[test]
    fn effects_default_is_strongest() {
        let e = Effects::default();
        assert_eq!(e.termination, Termination::Total);
        assert_eq!(e.completeness, Completeness::Complete);
        assert_eq!(e.determinism, Determinism::Deterministic);
    }
}
