//! Subsumption on terms / atoms. [Phase 3]
//!
//! One atom **subsumes** another when it is more general — there is a
//! substitution of the first's variables making it identical to the second
//! (`p(X, a)` subsumes `p(b, a)`; `p(X, X)` subsumes `p(a, a)` but not
//! `p(a, b)`). A fact store that keeps only *maximally general* facts (dropping
//! any atom subsumed by another) stays small when terms would otherwise pile up
//! near-duplicates — the pruning that makes term programs tractable.
//!
//! Subsumption is one-way matching (not unification): only the general side's
//! variables bind, the specific side is treated as ground structure.

use std::collections::HashMap;

/// A term with variables — the shape of a rule atom or a stored fact pattern.
/// An atom `p(t1, …, tn)` is a `Compound(p, [t1, …, tn])`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Term {
    Var(u32),
    Const(u32),
    Compound(u32, Vec<Term>),
}

impl Term {
    pub fn is_ground(&self) -> bool {
        match self {
            Term::Var(_) => false,
            Term::Const(_) => true,
            Term::Compound(_, a) => a.iter().all(Term::is_ground),
        }
    }
}

fn match_into<'a>(g: &Term, s: &'a Term, bind: &mut HashMap<u32, &'a Term>) -> bool {
    match g {
        Term::Var(v) => match bind.get(v) {
            Some(prev) => *prev == s, // a repeated variable must see the same term
            None => {
                bind.insert(*v, s);
                true
            }
        },
        Term::Const(c) => matches!(s, Term::Const(c2) if c == c2),
        Term::Compound(f, ga) => match s {
            Term::Compound(f2, sa) if f == f2 && ga.len() == sa.len() => {
                ga.iter().zip(sa).all(|(gi, si)| match_into(gi, si, bind))
            }
            _ => false,
        },
    }
}

/// Does `general` subsume `specific` — i.e. is there a substitution σ of
/// `general`'s variables with `general σ = specific`? `specific` is treated as
/// ground structure (its `Var`s only match a `general` `Var` bound to them).
pub fn subsumes(general: &Term, specific: &Term) -> bool {
    match_into(general, specific, &mut HashMap::new())
}

/// Insert `atom` into a maximally-general fact set: skip it if something already
/// present subsumes it, and drop anything it subsumes. Returns whether the set
/// changed. Keeps the store free of redundant, more-specific facts.
pub fn insert_maximal(store: &mut Vec<Term>, atom: Term) -> bool {
    if store.iter().any(|g| subsumes(g, &atom)) {
        return false;
    }
    store.retain(|s| !subsumes(&atom, s));
    store.push(atom);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(n: u32) -> Term {
        Term::Var(n)
    }
    fn c(n: u32) -> Term {
        Term::Const(n)
    }
    fn p(args: Vec<Term>) -> Term {
        Term::Compound(0, args)
    }

    #[test]
    fn basic_subsumption() {
        // p(X,a) subsumes p(b,a)
        assert!(subsumes(&p(vec![v(0), c(1)]), &p(vec![c(2), c(1)])));
        // p(X,X) subsumes p(a,a) but NOT p(a,b)
        assert!(subsumes(&p(vec![v(0), v(0)]), &p(vec![c(1), c(1)])));
        assert!(!subsumes(&p(vec![v(0), v(0)]), &p(vec![c(1), c(2)])));
        // a more specific atom does not subsume a more general one
        assert!(!subsumes(&p(vec![c(1), c(1)]), &p(vec![v(0), v(0)])));
        // different functor / arity
        assert!(!subsumes(&p(vec![v(0)]), &p(vec![c(1), c(2)])));
        // nested: f(X, g(Y)) subsumes f(a, g(b))
        let g_of = |t| Term::Compound(1, vec![t]);
        assert!(subsumes(
            &Term::Compound(2, vec![v(0), g_of(v(1))]),
            &Term::Compound(2, vec![c(9), g_of(c(8))]),
        ));
    }

    #[test]
    fn maximal_store_prunes_subsumed() {
        let mut store = Vec::new();
        // insert the general p(X,a)
        assert!(insert_maximal(&mut store, p(vec![v(0), c(1)])));
        // a specific p(b,a) is subsumed → not added
        assert!(!insert_maximal(&mut store, p(vec![c(2), c(1)])));
        assert_eq!(store.len(), 1);
        // an incomparable p(b,c) is added
        assert!(insert_maximal(&mut store, p(vec![c(2), c(3)])));
        assert_eq!(store.len(), 2);
        // inserting the even-more-general p(X,Y) drops both specifics
        assert!(insert_maximal(&mut store, p(vec![v(0), v(1)])));
        assert_eq!(store.len(), 1);
    }
}
