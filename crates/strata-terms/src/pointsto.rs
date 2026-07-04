//! Andersen points-to analysis — the Phase-3 exit. [spec §9]
//!
//! Inclusion-based (Andersen) points-to is the classic Datalog-with-terms static
//! analysis. Field sensitivity needs **structural terms**: a heap location is
//! `field(obj, f)`, interned to an id via [`crate::intern`] so equal locations
//! share one entry. The analysis is a semi-naive fixpoint over
//!
//! ```text
//! pt(p, o)  :- addrOf(p, o).                       // p = &o
//! pt(p, o)  :- copy(p, q), pt(q, o).               // p = q
//! fpt(field(o,f), o2) :- store(p, f, q), pt(p, o), pt(q, o2).   // p.f = q
//! pt(p, o2) :- load(p, q, f), pt(q, o), fpt(field(o,f), o2).    // p = q.f
//! ```
//!
//! It reports the fraction of time spent interning — the spec's success metric
//! ("приемлемая доля времени на интернирование").

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::intern::{Interner, TermId};

/// Functor id for the heap-location term `field(obj, f)`.
const FIELD: u32 = 0;

/// A program in points-to normal form (symbols are `u32` ids).
#[derive(Clone, Debug, Default)]
pub struct Program {
    /// `p = &o`
    pub addr_of: Vec<(u32, u32)>,
    /// `p = q`
    pub copy: Vec<(u32, u32)>,
    /// `p.f = q`
    pub store: Vec<(u32, u32, u32)>,
    /// `p = q.f`
    pub load: Vec<(u32, u32, u32)>,
}

/// The analysis result plus interning diagnostics.
pub struct Analysis {
    /// variable → set of objects it may point to.
    pub pt: HashMap<u32, HashSet<u32>>,
    /// interned `field(o,f)` → set of objects stored there.
    pub fpt: HashMap<TermId, HashSet<u32>>,
    pub interner: Interner,
    pub intern_time: Duration,
    pub total_time: Duration,
    pub rounds: u32,
}

impl Analysis {
    /// Fraction of wall-clock time spent interning heap-location terms.
    pub fn intern_fraction(&self) -> f64 {
        let t = self.total_time.as_secs_f64();
        if t > 0.0 {
            self.intern_time.as_secs_f64() / t
        } else {
            0.0
        }
    }
    /// Total number of `(var, object)` points-to pairs.
    pub fn pairs(&self) -> usize {
        self.pt.values().map(HashSet::len).sum()
    }
}

fn cst(i: &mut Interner, cache: &mut HashMap<u32, TermId>, it: &mut Duration, s: u32) -> TermId {
    if let Some(&x) = cache.get(&s) {
        return x;
    }
    let t0 = Instant::now();
    let x = i.intern_const(s);
    *it += t0.elapsed();
    cache.insert(s, x);
    x
}

fn field_term(
    i: &mut Interner,
    cache: &mut HashMap<u32, TermId>,
    it: &mut Duration,
    o: u32,
    f: u32,
) -> TermId {
    let co = cst(i, cache, it, o);
    let cf = cst(i, cache, it, f);
    let t0 = Instant::now();
    let x = i
        .intern_compound(FIELD, &[co, cf])
        .expect("field(o,f) is depth 2");
    *it += t0.elapsed();
    x
}

/// Run Andersen points-to to a fixpoint. `max_depth` bounds interned terms.
pub fn andersen(prog: &Program, max_depth: u32) -> Analysis {
    let start = Instant::now();
    let mut interner = Interner::new(max_depth);
    let mut cache: HashMap<u32, TermId> = HashMap::new();
    let mut it = Duration::ZERO;

    let mut pt: HashMap<u32, HashSet<u32>> = HashMap::new();
    let mut fpt: HashMap<TermId, HashSet<u32>> = HashMap::new();
    for &(p, o) in &prog.addr_of {
        pt.entry(p).or_default().insert(o);
    }

    let mut rounds = 0u32;
    loop {
        rounds += 1;
        let mut changed = false;

        // p = q
        for &(p, q) in &prog.copy {
            if let Some(src) = pt.get(&q).cloned() {
                let e = pt.entry(p).or_default();
                for o in src {
                    changed |= e.insert(o);
                }
            }
        }
        // p.f = q  →  fpt(field(o,f)) ⊇ pt(q) for o ∈ pt(p)
        for &(p, f, q) in &prog.store {
            let ps = pt.get(&p).cloned().unwrap_or_default();
            let qs = pt.get(&q).cloned().unwrap_or_default();
            for o in ps {
                let t = field_term(&mut interner, &mut cache, &mut it, o, f);
                let e = fpt.entry(t).or_default();
                for &o2 in &qs {
                    changed |= e.insert(o2);
                }
            }
        }
        // p = q.f  →  pt(p) ⊇ fpt(field(o,f)) for o ∈ pt(q)
        for &(p, q, f) in &prog.load {
            let qs = pt.get(&q).cloned().unwrap_or_default();
            for o in qs {
                let t = field_term(&mut interner, &mut cache, &mut it, o, f);
                if let Some(fs) = fpt.get(&t).cloned() {
                    let e = pt.entry(p).or_default();
                    for o2 in fs {
                        changed |= e.insert(o2);
                    }
                }
            }
        }

        if !changed {
            break;
        }
    }

    Analysis {
        pt,
        fpt,
        interner,
        intern_time: it,
        total_time: start.elapsed(),
        rounds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Symbol ids.
    const A: u32 = 0;
    const B: u32 = 1;
    const C: u32 = 2;
    const D: u32 = 3;
    const O1: u32 = 10;
    const O2: u32 = 11;
    const F: u32 = 100;

    fn set(xs: &[u32]) -> HashSet<u32> {
        xs.iter().copied().collect()
    }

    #[test]
    fn field_sensitive_pointsto() {
        // a = &o1;  b = a;  c = &o2;  a.f = c;  d = b.f;   ⇒ d points to o2.
        let prog = Program {
            addr_of: vec![(A, O1), (C, O2)],
            copy: vec![(B, A)],
            store: vec![(A, F, C)],
            load: vec![(D, B, F)],
        };
        let an = andersen(&prog, 8);
        assert_eq!(an.pt[&A], set(&[O1]));
        assert_eq!(an.pt[&B], set(&[O1]));
        assert_eq!(an.pt[&C], set(&[O2]));
        assert_eq!(an.pt[&D], set(&[O2]), "field flow o1.f = o2 reaches d");
    }

    #[test]
    fn copy_chain_is_transitive() {
        // a=&o1; b=a; c=b; d=c;  ⇒ all point to o1.
        let prog = Program {
            addr_of: vec![(A, O1)],
            copy: vec![(B, A), (C, B), (D, C)],
            ..Default::default()
        };
        let an = andersen(&prog, 8);
        for x in [A, B, C, D] {
            assert_eq!(an.pt[&x], set(&[O1]), "var {x}");
        }
    }

    #[test]
    fn two_objects_into_one_field() {
        // a=&o1; a.f=&o2; a.f=&o3(distinct); d=a.f  ⇒ d points to {o2,o3}.
        let (o3, e) = (12u32, 4u32);
        let prog = Program {
            addr_of: vec![(A, O1), (C, O2), (e, o3)],
            store: vec![(A, F, C), (A, F, e)],
            load: vec![(D, A, F)],
            ..Default::default()
        };
        let an = andersen(&prog, 8);
        assert_eq!(an.pt[&D], set(&[O2, o3]));
    }
}
