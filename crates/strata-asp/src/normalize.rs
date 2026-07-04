//! Normalizing choice rules and cardinalities to normal rules. [spec §1.6, §5.2]
//!
//! Spec §5.2: "normalization of choice rules and cardinalities — before
//! grounding, by the translations from §1.6". A choice/cardinality construct is
//! rewritten into ordinary normal rules + constraints so the rest of the
//! pipeline (grounding, aspif, clasp) only ever sees normal programs.
//!
//! - **Choice** `{a1; …; an} :- B` — each `ai` is independently in or out when
//!   `B` holds, via the even-loop complement: `ai :- B, not ãi.  ãi :- B, not ai`
//!   (`ãi` a fresh complement atom carrying `ai`'s arguments). First-order safe.
//! - **Cardinality** `l {g1; …; gn} u :- B` over a **ground** choice set — the
//!   choice above plus a Sinz-style sequential counter: aux atoms `c(i,j)` mean
//!   "≥ j of g1..gi hold". Then `:- B, not c(n,l)` enforces the lower bound and
//!   `:- B, c(n,u+1)` the upper bound. This is exactly the count normalization a
//!   grounder like gringo performs; here it is checked against clingo's native
//!   `l { … } u` (see `tests/clingo_diff.rs`).

use strata_ir::high::program::{Atom, Literal, Rule, Term};

/// A choice / cardinality rule: `lower { choices } upper :- body`.
/// `lower`/`upper` are `None` for an unbounded choice.
pub struct ChoiceRule {
    pub choices: Vec<Atom>,
    pub lower: Option<usize>,
    pub upper: Option<usize>,
    pub body: Vec<Literal>,
}

/// Generates fresh auxiliary predicate names, unique within a normalization run.
#[derive(Default)]
pub struct FreshGen {
    next: usize,
}
impl FreshGen {
    pub fn new() -> Self {
        FreshGen { next: 0 }
    }
    fn pred(&mut self, stem: &str) -> String {
        let n = self.next;
        self.next += 1;
        format!("__{stem}{n}")
    }
}

/// Normalize one choice/cardinality rule into normal rules + constraints.
/// Returns `(rules, constraints)` appended to the program's own.
pub fn normalize(cr: &ChoiceRule, fresh: &mut FreshGen) -> (Vec<Rule>, Vec<Vec<Literal>>) {
    let mut rules = Vec::new();
    let mut constraints = Vec::new();

    // 1. The choice itself: each choice atom independently in/out under `body`.
    let mut comps: Vec<Atom> = Vec::new();
    for ai in &cr.choices {
        let comp = Atom {
            pred: fresh.pred("nc"),
            args: ai.args.clone(),
        };
        comps.push(comp.clone());
        // ai :- body, not comp.
        let mut b1 = cr.body.clone();
        b1.push(Literal::Neg(comp.clone()));
        rules.push(Rule {
            head: ai.clone(),
            body: b1,
        });
        // comp :- body, not ai.
        let mut b2 = cr.body.clone();
        b2.push(Literal::Neg(ai.clone()));
        rules.push(Rule {
            head: comp,
            body: b2,
        });
    }

    // 2. Cardinality bounds via a sequential counter over the (ground) choices.
    if cr.lower.is_some() || cr.upper.is_some() {
        assert!(
            cr.choices.iter().all(is_ground_atom),
            "cardinality bounds require ground choice atoms"
        );
        let n = cr.choices.len();
        let maxj = cr
            .upper
            .map(|u| u + 1)
            .unwrap_or(0)
            .max(cr.lower.unwrap_or(0));

        // c(i,j) atoms, distinct 0-ary predicates keyed by (i,j).
        let cid = fresh.pred("c");
        let c = |i: usize, j: usize| Atom {
            pred: format!("{cid}_{i}_{j}"),
            args: vec![],
        };

        for i in 1..=n {
            let gi = cr.choices[i - 1].clone();
            for j in 1..=maxj {
                // carry: c(i,j) :- c(i-1,j)   (i>1)
                if i > 1 {
                    rules.push(Rule {
                        head: c(i, j),
                        body: vec![Literal::Pos(c(i - 1, j))],
                    });
                }
                // contribute: c(i,j) :- [c(i-1,j-1),] gi
                let mut body = Vec::new();
                if j > 1 {
                    if i == 1 {
                        continue; // c(1,j>1) impossible
                    }
                    body.push(Literal::Pos(c(i - 1, j - 1)));
                }
                body.push(Literal::Pos(gi.clone()));
                rules.push(Rule {
                    head: c(i, j),
                    body,
                });
            }
        }

        if let Some(l) = cr.lower {
            if l > 0 {
                // :- body, not c(n,l)
                let mut con = cr.body.clone();
                con.push(Literal::Neg(c(n, l)));
                constraints.push(con);
            }
        }
        if let Some(u) = cr.upper {
            // :- body, c(n,u+1)
            let mut con = cr.body.clone();
            con.push(Literal::Pos(c(n, u + 1)));
            constraints.push(con);
        }
    }

    (rules, constraints)
}

fn is_ground_atom(a: &Atom) -> bool {
    a.args
        .iter()
        .all(|t| matches!(t, Term::Const { .. } | Term::Int { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{solve, Val};
    use strata_ir::high::program::atom;

    fn gatom(pred: &str) -> Atom {
        atom(pred, vec![])
    }

    /// Solve a normalized choice rule (no extra program) and count models.
    fn count_models(cr: &ChoiceRule) -> usize {
        let mut fresh = FreshGen::new();
        let (rules, cons) = normalize(cr, &mut fresh);
        solve(&rules, &[], &cons).unwrap().len()
    }

    #[test]
    fn unbounded_choice_is_powerset() {
        // {a; b; c}. ⇒ 2^3 = 8 answer sets (over the choice atoms).
        let cr = ChoiceRule {
            choices: vec![gatom("a"), gatom("b"), gatom("c")],
            lower: None,
            upper: None,
            body: vec![],
        };
        assert_eq!(count_models(&cr), 8);
    }

    #[test]
    fn exactly_one() {
        // 1 {a; b; c} 1. ⇒ 3 answer sets.
        let cr = ChoiceRule {
            choices: vec![gatom("a"), gatom("b"), gatom("c")],
            lower: Some(1),
            upper: Some(1),
            body: vec![],
        };
        assert_eq!(count_models(&cr), 3);
    }

    #[test]
    fn at_least_two_of_three() {
        // 2 {a;b;c}. ⇒ subsets of size ≥2 = {ab,ac,bc,abc} = 4.
        let cr = ChoiceRule {
            choices: vec![gatom("a"), gatom("b"), gatom("c")],
            lower: Some(2),
            upper: None,
            body: vec![],
        };
        assert_eq!(count_models(&cr), 4);
    }

    #[test]
    fn at_most_one_of_three() {
        // {a;b;c} 1. ⇒ subsets of size ≤1 = {∅,a,b,c} = 4.
        let cr = ChoiceRule {
            choices: vec![gatom("a"), gatom("b"), gatom("c")],
            lower: None,
            upper: Some(1),
            body: vec![],
        };
        assert_eq!(count_models(&cr), 4);
    }

    #[test]
    fn exactly_two_of_four() {
        // 2 {a;b;c;d} 2. ⇒ C(4,2) = 6.
        let cr = ChoiceRule {
            choices: vec![gatom("a"), gatom("b"), gatom("c"), gatom("d")],
            lower: Some(2),
            upper: Some(2),
            body: vec![],
        };
        assert_eq!(count_models(&cr), 6);
    }

    #[test]
    fn choice_carries_args() {
        // {p(x)}. over a ground arg ⇒ 2 models: {} and {p(x)}.
        let cr = ChoiceRule {
            choices: vec![atom("p", vec![Term::Const { name: "x".into() }])],
            lower: None,
            upper: None,
            body: vec![],
        };
        let mut fresh = FreshGen::new();
        let (rules, cons) = normalize(&cr, &mut fresh);
        let models = solve(&rules, &[], &cons).unwrap();
        assert_eq!(models.len(), 2);
        let px: (String, Vec<Val>) = ("p".into(), vec![Val::Sym("x".into())]);
        assert!(models.iter().any(|m| m.contains(&px)));
        assert!(models.iter().any(|m| !m.contains(&px)));
    }
}
