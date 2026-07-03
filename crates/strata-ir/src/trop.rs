//! Tropical (min,+) weight scalar. [IR-3, D6, spec 2.4, invariant I5]
//!
//! A weight is `i64` with a **distinct** `+∞` sentinel — never `i64::MAX`, so no
//! finite arithmetic result can collide with `+∞`. `⊕` is `min` and `⊗` is
//! checked `+` where finite overflow is a **runtime error, not saturation**.
//! Because `min` and integer `+` are exactly associative, GPU↔CPU differential
//! tests are bit-for-bit with no epsilon.
//!
//! JSON encoding (see docs/ir-encoding.md rule 5): finite → bare integer,
//! `+∞` → the string `"inf"`.

use serde::de::{self, Deserialize, Deserializer};
use serde::ser::{Serialize, Serializer};

/// Overflow of finite `⊗` (checked `i64` addition). Surfaces as a runtime error
/// in eval (D6), never wraps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TropOverflow {
    pub lhs: i64,
    pub rhs: i64,
}

impl core::fmt::Display for TropOverflow {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "tropical weight overflow: {} + {} exceeds i64",
            self.lhs, self.rhs
        )
    }
}

/// A tropical weight. Ordering is total with every finite value below `PosInf`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Weight {
    Finite(i64),
    /// `+∞`: the `⊕`(min) identity and the `⊗`(+) annihilator.
    PosInf,
}

impl Weight {
    /// `⊕` identity (neutral for `min`).
    pub const OPLUS_ID: Weight = Weight::PosInf;
    /// `⊗` identity (neutral for `+`); also how a Bool `true` enters Trop (spec 1.7).
    pub const OTIMES_ID: Weight = Weight::Finite(0);

    /// Semiring `⊕`: `min`. Associative, commutative, idempotent.
    #[must_use]
    pub fn oplus(self, other: Weight) -> Weight {
        self.min(other)
    }

    /// Semiring `⊗`: checked `+` with `+∞` absorption. `Err` on finite overflow.
    pub fn otimes(self, other: Weight) -> Result<Weight, TropOverflow> {
        match (self, other) {
            (Weight::PosInf, _) | (_, Weight::PosInf) => Ok(Weight::PosInf),
            (Weight::Finite(a), Weight::Finite(b)) => a
                .checked_add(b)
                .map(Weight::Finite)
                .ok_or(TropOverflow { lhs: a, rhs: b }),
        }
    }

    pub fn is_inf(self) -> bool {
        matches!(self, Weight::PosInf)
    }
}

impl Serialize for Weight {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Weight::Finite(n) => s.serialize_i64(*n),
            Weight::PosInf => s.serialize_str(crate::output::POS_INF_TOKEN),
        }
    }
}

impl<'de> Deserialize<'de> for Weight {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl de::Visitor<'_> for V {
            type Value = Weight;
            fn expecting(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
                write!(
                    f,
                    "an i64 weight or the string \"{}\"",
                    crate::output::POS_INF_TOKEN
                )
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Weight, E> {
                Ok(Weight::Finite(v))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Weight, E> {
                i64::try_from(v)
                    .map(Weight::Finite)
                    .map_err(de::Error::custom)
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Weight, E> {
                if v == crate::output::POS_INF_TOKEN {
                    Ok(Weight::PosInf)
                } else {
                    Err(de::Error::custom(format!("invalid weight string: {v:?}")))
                }
            }
        }
        d.deserialize_any(V)
    }
}

impl schemars::JsonSchema for Weight {
    fn schema_name() -> String {
        "Weight".to_string()
    }
    fn json_schema(_gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        use schemars::schema::{InstanceType, SchemaObject, SubschemaValidation};
        let integer = SchemaObject {
            instance_type: Some(InstanceType::Integer.into()),
            ..Default::default()
        };
        let inf = SchemaObject {
            instance_type: Some(InstanceType::String.into()),
            const_value: Some(serde_json::json!(crate::output::POS_INF_TOKEN)),
            ..Default::default()
        };
        SchemaObject {
            subschemas: Some(Box::new(SubschemaValidation {
                one_of: Some(vec![integer.into(), inf.into()]),
                ..Default::default()
            })),
            ..Default::default()
        }
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_order_finite_below_inf() {
        assert!(Weight::Finite(i64::MAX) < Weight::PosInf);
        assert!(Weight::Finite(-5) < Weight::Finite(3));
    }

    #[test]
    fn identities() {
        let x = Weight::Finite(7);
        assert_eq!(x.oplus(Weight::OPLUS_ID), x); // min(x, +inf) = x
        assert_eq!(x.otimes(Weight::OTIMES_ID).unwrap(), x); // x + 0 = x
    }

    #[test]
    fn otimes_inf_absorbs() {
        assert_eq!(
            Weight::Finite(3).otimes(Weight::PosInf).unwrap(),
            Weight::PosInf
        );
        assert_eq!(
            Weight::PosInf.otimes(Weight::PosInf).unwrap(),
            Weight::PosInf
        );
    }

    #[test]
    fn otimes_overflow_is_error_not_wrap() {
        let r = Weight::Finite(i64::MAX).otimes(Weight::Finite(1));
        assert_eq!(
            r,
            Err(TropOverflow {
                lhs: i64::MAX,
                rhs: 1
            })
        );
    }

    #[test]
    fn associative_commutative_sample() {
        let vals = [
            Weight::Finite(-3),
            Weight::Finite(0),
            Weight::Finite(9),
            Weight::PosInf,
        ];
        for &a in &vals {
            for &b in &vals {
                assert_eq!(a.oplus(b), b.oplus(a));
                for &c in &vals {
                    assert_eq!(a.oplus(b).oplus(c), a.oplus(b.oplus(c)));
                    // ⊗ associative where no overflow (small values chosen).
                    if let (Ok(ab), Ok(bc)) = (a.otimes(b), b.otimes(c)) {
                        assert_eq!(ab.otimes(c), a.otimes(bc));
                    }
                }
            }
        }
    }

    #[test]
    fn json_roundtrip() {
        let fin = Weight::Finite(42);
        let inf = Weight::PosInf;
        assert_eq!(serde_json::to_string(&fin).unwrap(), "42");
        assert_eq!(serde_json::to_string(&inf).unwrap(), "\"inf\"");
        assert_eq!(serde_json::from_str::<Weight>("42").unwrap(), fin);
        assert_eq!(serde_json::from_str::<Weight>("\"inf\"").unwrap(), inf);
        // +inf never parses back as a finite integer.
        assert!(serde_json::from_str::<Weight>("\"nan\"").is_err());
    }
}
