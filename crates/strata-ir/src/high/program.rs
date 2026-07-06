//! High-IR program AST: the public, LLM-writable source of truth. [IR-4, D2, D4]
//!
//! Represents the WHOLE language surface (D5): the executable Bool/Trop fragment
//! plus structurally-present-but-inert constructs (neural predicates,
//! probabilistic facts, `@terms`/`@asp` pragmas, `?prob`/`?grad` queries) that
//! downstream stages gate with an honest "not implemented in Phase 0" error.
//!
//! Top-level order is preserved as `items: Vec<Item>` so the canonical printer is
//! a faithful projection. Each item carries optional [`Trivia`]; per the IR-5
//! contract, trivia is excluded from equality.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::sig::Signature;
use super::trivia::Trivia;
use crate::diag::Span;
use crate::trop::Weight;

/// A whole program document. Carries its `ir_version` (D2/D11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Program {
    pub ir_version: String,
    pub items: Vec<Item>,
}

impl Program {
    /// A program stamped with the current build's IR version.
    pub fn new(items: Vec<Item>) -> Self {
        Self {
            ir_version: crate::IR_VERSION_STR.to_string(),
            items,
        }
    }
}

/// A top-level statement plus its attached trivia and source span.
///
/// **Equality excludes trivia and span** (IR-5 contract): two `Item`s with the
/// same `node` are equal regardless of comments or source location. The span is
/// clause-level provenance the checker uses to point diagnostics at source.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Item {
    #[serde(default, skip_serializing_if = "Trivia::is_empty")]
    pub trivia: Trivia,
    #[serde(default, skip_serializing_if = "Span::is_zero")]
    pub span: Span,
    #[serde(flatten)]
    pub node: ItemKind,
}

impl Item {
    pub fn new(node: ItemKind) -> Self {
        Self {
            trivia: Trivia::default(),
            span: Span::default(),
            node,
        }
    }

    /// An item with an explicit clause span (set by the parser).
    pub fn with_span(node: ItemKind, span: Span) -> Self {
        Self {
            trivia: Trivia::default(),
            span,
            node,
        }
    }
}

impl PartialEq for Item {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node // trivia and span deliberately ignored (IR-5)
    }
}

/// The statement kinds. Adjacently tagged (docs/ir-encoding.md rule 2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ItemKind {
    Domain(DomainDecl),
    Predicate(PredDecl),
    Rule(Rule),
    Fact(Fact),
    Input(InputDecl),
    Query(Query),
    Pragma(Pragma),
}

/// A declared constant domain (`.type`-like).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DomainDecl {
    pub name: String,
}

/// A mandatory predicate declaration (D3): name + signature (+ optional neural
/// binding, inert in Phase 0 per D5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PredDecl {
    pub name: String,
    pub sig: Signature,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub neural: Option<NeuralSpec>,
}

/// `neural p(...) from model "..."` — parsed, never executed in Phase 0 (D5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NeuralSpec {
    pub model: String,
}

/// A rule `H :- B1, ..., Bn.`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Rule {
    pub head: Atom,
    pub body: Vec<Literal>,
}

/// A predicate application. Head aggregates live as [`Term::Agg`] head args.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Atom {
    pub pred: String,
    pub args: Vec<Term>,
}

/// A body literal: positive or (stratified) negated atom. [spec 1.2]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Literal {
    Pos(Atom),
    Neg(Atom),
}

/// An argument term.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Term {
    /// Uppercase/`_`-leading identifier (Prolog convention, D3).
    Var { name: String },
    /// Lowercase identifier — a constant symbol.
    Const { name: String },
    /// Integer literal (also a Trop weight source, D6).
    Int { value: i64 },
    /// Aggregate head term `agg⟨Var⟩` (spec 1.3).
    Agg { op: AggOp, var: String },
    /// Constructor term `functor(args...)` (`@terms`, spec §1.4).
    Compound { functor: String, args: Vec<Term> },
}

/// Aggregate operators (spec 1.3). Pure-unit enum → bare string (rule 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AggOp {
    Min,
    Max,
    Sum,
    Count,
    ProbOr,
}

/// A fact. `weight` present ⇒ tropical; `prob` present ⇒ probabilistic (inert, D5).
/// Both absent ⇒ a plain Bool fact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Fact {
    pub atom: Atom,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<Weight>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prob: Option<f64>,
}

/// `input pred from "file.tsv"` — Soufflé-compatible TSV EDB (D10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InputDecl {
    pub pred: String,
    pub path: String,
}

/// A query. `Prob`/`Grad` are parsed but not executed in Phase 0 (D5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Query {
    pub atom: Atom,
    pub kind: QueryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueryKind {
    Plain,
    Prob,
    Grad,
}

/// A module pragma. `@terms`/`@asp` — parsed, gated as not-implemented (D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Pragma {
    Terms,
    Asp,
}

// --- small constructors for hand-authoring fixtures/tests -------------------

/// `Term::Var`
pub fn var(name: &str) -> Term {
    Term::Var {
        name: name.to_string(),
    }
}
/// `Term::Const`
pub fn cst(name: &str) -> Term {
    Term::Const {
        name: name.to_string(),
    }
}
/// An atom from a predicate name and terms.
pub fn atom(pred: &str, args: Vec<Term>) -> Atom {
    Atom {
        pred: pred.to_string(),
        args,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equality_ignores_trivia() {
        let a = Item::new(ItemKind::Domain(DomainDecl {
            name: "node".into(),
        }));
        let mut b = a.clone();
        b.trivia.leading.push("% a comment".into());
        b.trivia.blank_before = true;
        assert_eq!(a, b, "IR-5: comments must not affect semantic equality");
    }
}
