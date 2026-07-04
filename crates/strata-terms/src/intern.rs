//! Host interning of ground terms, with a depth bound. [Phase 3]
//!
//! A ground term is a symbol (`Const`) or a functor applied to previously
//! interned terms (`Compound`). Interning hash-conses each distinct term to a
//! small integer id, so equal sub-terms are shared and equality is an `==` on
//! ids — the substrate a Datalog engine needs to store terms in relations. Each
//! term carries its nesting depth; `intern_compound` refuses to build past
//! `max_depth`, which is what makes a term-generating recursion terminate
//! (`list(cons(H, T)) :- ...` cannot grow unboundedly).

use std::collections::HashMap;

/// A symbol id (nullary constant / functor name).
pub type Sym = u32;
/// A functor id.
pub type Functor = u32;
/// An interned term id.
pub type TermId = u32;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Node {
    Const(Sym),
    Compound(Functor, Vec<TermId>),
}

/// A term nested deeper than the interner's `max_depth` was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepthExceeded {
    pub depth: u32,
    pub max: u32,
}

impl std::fmt::Display for DepthExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "term depth {} exceeds the bound {}",
            self.depth, self.max
        )
    }
}
impl std::error::Error for DepthExceeded {}

/// A depth-bounded, hash-consing interner of ground terms.
pub struct Interner {
    map: HashMap<Node, TermId>,
    nodes: Vec<Node>,
    depth: Vec<u32>,
    max_depth: u32,
    /// Number of *new* terms actually created (hash-cons misses) — for profiling.
    created: u64,
    /// Number of `intern_*` calls — for the interning-time-fraction metric.
    calls: u64,
}

impl Interner {
    /// A fresh interner rejecting terms deeper than `max_depth`.
    pub fn new(max_depth: u32) -> Self {
        Interner {
            map: HashMap::new(),
            nodes: Vec::new(),
            depth: Vec::new(),
            max_depth,
            created: 0,
            calls: 0,
        }
    }

    fn node(&mut self, n: Node, depth: u32) -> TermId {
        if let Some(&id) = self.map.get(&n) {
            return id;
        }
        let id = self.nodes.len() as TermId;
        self.map.insert(n.clone(), id);
        self.nodes.push(n);
        self.depth.push(depth);
        self.created += 1;
        id
    }

    /// Intern a constant (depth 0). Always succeeds.
    pub fn intern_const(&mut self, s: Sym) -> TermId {
        self.calls += 1;
        self.node(Node::Const(s), 0)
    }

    /// Intern `f(args...)`. Its depth is `1 + max(arg depths)`; rejected if that
    /// exceeds `max_depth`.
    pub fn intern_compound(
        &mut self,
        f: Functor,
        args: &[TermId],
    ) -> Result<TermId, DepthExceeded> {
        self.calls += 1;
        let depth = 1 + args
            .iter()
            .map(|&a| self.depth[a as usize])
            .max()
            .unwrap_or(0);
        if depth > self.max_depth {
            return Err(DepthExceeded {
                depth,
                max: self.max_depth,
            });
        }
        Ok(self.node(Node::Compound(f, args.to_vec()), depth))
    }

    /// Nesting depth of an interned term.
    pub fn depth_of(&self, t: TermId) -> u32 {
        self.depth[t as usize]
    }

    /// Distinct terms interned so far.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// (`intern_*` calls, distinct terms created) — hash-cons hit rate diagnostics.
    pub fn stats(&self) -> (u64, u64) {
        (self.calls, self.created)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_cons_shares_structure() {
        let mut i = Interner::new(64);
        let a = i.intern_const(0);
        let b = i.intern_const(1);
        let nil = i.intern_const(2);
        // cons(a, cons(b, nil)) built twice must give the same id.
        let inner1 = i.intern_compound(10, &[b, nil]).unwrap();
        let l1 = i.intern_compound(10, &[a, inner1]).unwrap();
        let inner2 = i.intern_compound(10, &[b, nil]).unwrap();
        let l2 = i.intern_compound(10, &[a, inner2]).unwrap();
        assert_eq!(inner1, inner2);
        assert_eq!(l1, l2);
        // consts dedup too
        assert_eq!(a, i.intern_const(0));
        // 5 distinct terms: a, b, nil, cons(b,nil), cons(a,..)
        assert_eq!(i.len(), 5);
    }

    #[test]
    fn depth_is_tracked_and_bounded() {
        let mut i = Interner::new(3);
        let z = i.intern_const(0); // depth 0
        assert_eq!(i.depth_of(z), 0);
        let s1 = i.intern_compound(1, &[z]).unwrap(); // depth 1
        let s2 = i.intern_compound(1, &[s1]).unwrap(); // depth 2
        let s3 = i.intern_compound(1, &[s2]).unwrap(); // depth 3 (== max, ok)
        assert_eq!(i.depth_of(s3), 3);
        // depth 4 exceeds the bound → rejected (this is what stops the recursion)
        let err = i.intern_compound(1, &[s3]).unwrap_err();
        assert_eq!(err, DepthExceeded { depth: 4, max: 3 });
    }

    #[test]
    fn depth_takes_the_max_over_args() {
        let mut i = Interner::new(64);
        let z = i.intern_const(0);
        let s1 = i.intern_compound(1, &[z]).unwrap(); // depth 1
        let s2 = i.intern_compound(1, &[s1]).unwrap(); // depth 2
        let pair = i.intern_compound(2, &[z, s2]).unwrap(); // 1 + max(0,2) = 3
        assert_eq!(i.depth_of(pair), 3);
    }
}
