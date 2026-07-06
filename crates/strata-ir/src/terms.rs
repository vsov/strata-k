//! Ground structural terms via hash-consing. [spec §1.4 `@terms`]
//!
//! `@terms` adds constructor terms (`cons(H, T)`, `node(L, V, R)`): the language
//! becomes Turing-complete and loses the guaranteed-termination property. The
//! implementation is a global hash-cons — every ground compound term is
//! canonicalized to a scalar integer id ([`TermId`]) so the join engine only ever
//! compares scalars ([`crate::value::GroundVal::Term`] holds the id), and equal
//! terms share one id (spec §1.4). Divergence is bounded by an optional
//! `max_depth`: interning a term deeper than the bound is refused with
//! [`DepthExceeded`], which the evaluator turns into a dropped derivation and a
//! *sound-but-incomplete* result status (spec §1.4, §3.4 — the `Sound[T]` type).

use std::collections::HashMap;

use crate::dict::SymbolId;
use crate::value::GroundVal;

/// Default divergence bound for `@terms` modules: a constructed term deeper than
/// this is dropped, leaving the answer sound but flagged incomplete (spec §1.4).
/// Shared by the checker (which builds the table) and the CLI (which extends it).
pub const DEFAULT_MAX_DEPTH: u32 = 64;

/// A scalar id for a hash-consed ground compound term.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
pub struct TermId(pub u32);

/// Interning a term exceeded the module's declared depth bound (spec §1.4). The
/// answer stays sound (terms are only *dropped*, never invented); completeness
/// is what is lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepthExceeded {
    pub depth: u32,
    pub max: u32,
}

/// A node: functor, arguments (leaves or nested term ids), and cached depth.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    functor: SymbolId,
    args: Vec<GroundVal>,
    depth: u32,
}

/// The global hash-cons table for one evaluation. `max_depth == 0` means
/// unbounded (the program declared no `@depth` bound).
#[derive(Debug, Clone, Default)]
pub struct TermTable {
    nodes: Vec<Node>,
    intern: HashMap<(SymbolId, Vec<GroundVal>), u32>,
    max_depth: u32,
    dropped: u64,
}

impl TermTable {
    pub fn new(max_depth: u32) -> Self {
        TermTable {
            nodes: Vec::new(),
            intern: HashMap::new(),
            max_depth,
            dropped: 0,
        }
    }

    /// Record that a derivation was dropped because a constructed term exceeded
    /// the depth bound (spec §1.4): the result is now sound but possibly
    /// incomplete.
    pub fn note_drop(&mut self) {
        self.dropped += 1;
    }
    /// How many derivations were dropped at the depth bound.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
    /// Whether the result is complete (no term was ever dropped).
    pub fn is_complete(&self) -> bool {
        self.dropped == 0
    }

    /// The depth of a value: a leaf (`Sym`/`Int`) is `0`; a compound is
    /// `1 + max(arg depth)`.
    pub fn value_depth(&self, v: GroundVal) -> u32 {
        match v {
            GroundVal::Sym(_) | GroundVal::Int(_) => 0,
            GroundVal::Term(id) => self.nodes[id.0 as usize].depth,
        }
    }

    /// Intern a compound term `functor(args...)`, returning its canonical id.
    /// Equal terms return the same id. Refused with [`DepthExceeded`] if the
    /// resulting depth exceeds a nonzero `max_depth`.
    pub fn intern(
        &mut self,
        functor: SymbolId,
        args: Vec<GroundVal>,
    ) -> Result<TermId, DepthExceeded> {
        let depth = 1 + args.iter().map(|&a| self.value_depth(a)).max().unwrap_or(0);
        if self.max_depth != 0 && depth > self.max_depth {
            return Err(DepthExceeded {
                depth,
                max: self.max_depth,
            });
        }
        let key = (functor, args.clone());
        if let Some(&id) = self.intern.get(&key) {
            return Ok(TermId(id));
        }
        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            functor,
            args,
            depth,
        });
        self.intern.insert(key, id);
        Ok(TermId(id))
    }

    /// The functor and arguments of a term id (for unification / printing).
    pub fn get(&self, id: TermId) -> (SymbolId, &[GroundVal]) {
        let n = &self.nodes[id.0 as usize];
        (n.functor, &n.args)
    }

    /// Number of interned terms.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(n: u32) -> SymbolId {
        SymbolId(n)
    }

    #[test]
    fn equal_terms_share_an_id() {
        let mut t = TermTable::new(0);
        let a = t.intern(sym(1), vec![GroundVal::Int(1)]).unwrap();
        let b = t.intern(sym(1), vec![GroundVal::Int(1)]).unwrap();
        let c = t.intern(sym(1), vec![GroundVal::Int(2)]).unwrap();
        assert_eq!(a, b, "hash-consing must reuse the id");
        assert_ne!(a, c);
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn depth_is_structural() {
        let mut t = TermTable::new(0);
        // cons(1, cons(2, nil))  — depth 2 (nil is a leaf const here).
        let inner = t
            .intern(sym(9), vec![GroundVal::Int(2), GroundVal::Sym(sym(0))])
            .unwrap();
        assert_eq!(t.value_depth(GroundVal::Term(inner)), 1);
        let outer = t
            .intern(sym(9), vec![GroundVal::Int(1), GroundVal::Term(inner)])
            .unwrap();
        assert_eq!(t.value_depth(GroundVal::Term(outer)), 2);
    }

    #[test]
    fn depth_bound_refuses_deep_terms() {
        let mut t = TermTable::new(1);
        // depth-1 ok:
        let ok = t.intern(sym(9), vec![GroundVal::Int(1)]).unwrap();
        // depth-2 refused:
        let err = t.intern(sym(9), vec![GroundVal::Term(ok)]);
        assert_eq!(err, Err(DepthExceeded { depth: 2, max: 1 }));
    }
}
