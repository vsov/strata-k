//! `strata-asp` — a reference answer-set (stable model) solver. [Phase 5, spec 5]
//!
//! **Stability: experimental.** Declarations are enforced (`E1001`/`E1005`/
//! `E1011`/`E1012`), but the `@asp` surface is narrower than the deductive core
//! and still settling. See the Stability section of `docs/language.md` for the
//! stable kernel.
//!
//! The obviously-correct reference (the ASP analogue of the naive `T_P` oracle),
//! against which a future clasp-backed / GPU-grounded solver must agree:
//!
//! 1. **Naive grounding.** Every rule is instantiated over the Herbrand universe
//!    (the constants of the program). Spec §5.2's "intelligent grounding" is an
//!    optimization; this is the ground truth.
//! 2. **Stable models via the Gelfond–Lifschitz reduct.** A set `S` is a stable
//!    model iff `S` is the least model of the reduct `P^S` (drop each rule with a
//!    `not c`, c∈S; then drop the remaining negative literals). Because the
//!    reduct depends only on which *negated* atoms are in `S`, we enumerate
//!    guesses over the negated atoms alone (bounded by `2^|N|`, ≪ `2^|HB|`),
//!    compute the least model, and confirm it reproduces the guess.
//! 3. **Constraints** `:- B` are headless rules: an answer set may not satisfy
//!    any constraint body.
//!
//! Normal programs only (no disjunction), per spec §1.6 / §8.

use std::collections::{BTreeSet, HashMap, HashSet};

use strata_ir::high::program::{Atom, Literal, Rule, Term};

pub mod aspif;
pub mod clasp;
pub mod heuristic;
pub mod normalize;
pub mod simplify;
pub mod unfounded;

/// A ground constant value (self-contained; no symbol dictionary needed here).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Val {
    Sym(String),
    Int(i64),
}

/// A ground atom: predicate + ground arguments.
pub type GroundAtom = (String, Vec<Val>);

/// Guessing is refused past this many distinct negated ground atoms (`2^n`).
pub const MAX_GUESS_ATOMS: usize = 24;
/// Grounding is refused past this many instantiations of a single rule.
pub const MAX_INSTANTIATIONS: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AspError {
    /// Too many negated atoms for exact enumeration (ΣP-hard territory).
    TooManyChoices(usize),
    /// A single rule grounds to more instantiations than allowed.
    TooLargeGrounding,
    /// A construct outside the reference solver's scope (e.g. an aggregate atom).
    Unsupported(&'static str),
}

impl std::fmt::Display for AspError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AspError::TooManyChoices(n) => {
                write!(
                    f,
                    "{n} negated atoms exceed the guess limit of {MAX_GUESS_ATOMS}"
                )
            }
            AspError::TooLargeGrounding => {
                write!(f, "rule grounds too large for the reference solver")
            }
            AspError::Unsupported(w) => write!(f, "not supported by the reference ASP solver: {w}"),
        }
    }
}

impl std::error::Error for AspError {}

// --- ground representation ---------------------------------------------------

#[derive(Default)]
struct Interner {
    map: HashMap<GroundAtom, usize>,
    list: Vec<GroundAtom>,
}
impl Interner {
    fn intern(&mut self, a: GroundAtom) -> usize {
        if let Some(&id) = self.map.get(&a) {
            return id;
        }
        let id = self.list.len();
        self.list.push(a.clone());
        self.map.insert(a, id);
        id
    }
}

/// A ground rule over interned atom ids: `head :- pos, not neg`.
pub struct GRule {
    /// `None` ⇒ a constraint (`:- body`, an empty/false head).
    pub head: Option<usize>,
    pub pos: Vec<usize>,
    pub neg: Vec<usize>,
}

/// A fully grounded normal program: the ground rules plus the atom table that
/// maps each interned id back to its ground atom. This is the hand-off point
/// shared by the reference solver, the aspif emitter (spec §5.2), and the
/// unfounded-set verifier (spec §5.3).
pub struct Ground {
    pub rules: Vec<GRule>,
    /// `atoms[id]` is the ground atom interned as `id`.
    pub atoms: Vec<GroundAtom>,
}

impl Ground {
    /// Number of distinct ground atoms.
    pub fn n_atoms(&self) -> usize {
        self.atoms.len()
    }
}

// --- public entry point ------------------------------------------------------

/// Ground a normal program (facts + rules + constraints) over its Herbrand
/// universe into interned ground rules. The obviously-correct naive grounding;
/// spec §5.2's GPU "intelligent grounding" is an optimization checked against it.
pub fn ground(
    rules: &[Rule],
    facts: &[GroundAtom],
    constraints: &[Vec<Literal>],
) -> Result<Ground, AspError> {
    let universe = herbrand_universe(rules, facts, constraints);
    let mut intern = Interner::default();
    let mut grules: Vec<GRule> = Vec::new();

    for (pred, args) in facts {
        let id = intern.intern((pred.clone(), args.clone()));
        grules.push(GRule {
            head: Some(id),
            pos: Vec::new(),
            neg: Vec::new(),
        });
    }
    for r in rules {
        ground_rule(r, &universe, &mut intern, &mut grules)?;
    }
    for body in constraints {
        ground_constraint(body, &universe, &mut intern, &mut grules)?;
    }
    Ok(Ground {
        rules: grules,
        atoms: intern.list,
    })
}

/// Compute all stable models of a normal program (rules + facts + constraints),
/// each returned as a sorted list of ground atoms. Results are sorted for a
/// deterministic order.
pub fn solve(
    rules: &[Rule],
    facts: &[GroundAtom],
    constraints: &[Vec<Literal>],
) -> Result<Vec<Vec<GroundAtom>>, AspError> {
    let g = ground(rules, facts, constraints)?;
    let models = stable_models(g.atoms.len(), &g.rules)?;
    Ok(resolve_models(&g, models))
}

/// Stable models of an already-grounded program (e.g. the output of
/// [`simplify::simplify`]), as sorted ground-atom lists.
pub fn stable_models_of(g: &Ground) -> Result<Vec<Vec<GroundAtom>>, AspError> {
    let models = stable_models(g.atoms.len(), &g.rules)?;
    Ok(resolve_models(g, models))
}

/// Resolve interned model ids back to sorted ground atoms, sorted for determinism.
pub(crate) fn resolve_models(g: &Ground, models: Vec<BTreeSet<usize>>) -> Vec<Vec<GroundAtom>> {
    let mut out: Vec<Vec<GroundAtom>> = models
        .into_iter()
        .map(|s| {
            let mut atoms: Vec<GroundAtom> = s.iter().map(|&id| g.atoms[id].clone()).collect();
            atoms.sort();
            atoms
        })
        .collect();
    out.sort();
    out
}

// --- grounding ---------------------------------------------------------------

fn herbrand_universe(
    rules: &[Rule],
    facts: &[GroundAtom],
    constraints: &[Vec<Literal>],
) -> Vec<Val> {
    let mut set: BTreeSet<Val> = BTreeSet::new();
    let from_atom = |a: &Atom, set: &mut BTreeSet<Val>| {
        for t in &a.args {
            match t {
                Term::Const { name } => {
                    set.insert(Val::Sym(name.clone()));
                }
                Term::Int { value } => {
                    set.insert(Val::Int(*value));
                }
                _ => {}
            }
        }
    };
    for (_, args) in facts {
        for v in args {
            set.insert(v.clone());
        }
    }
    for r in rules {
        from_atom(&r.head, &mut set);
        for lit in &r.body {
            from_atom(literal_atom(lit), &mut set);
        }
    }
    for body in constraints {
        for lit in body {
            from_atom(literal_atom(lit), &mut set);
        }
    }
    set.into_iter().collect()
}

fn rule_vars(head: Option<&Atom>, body: &[Literal]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let collect = |a: &Atom, seen: &mut BTreeSet<String>| {
        for t in &a.args {
            if let Term::Var { name } = t {
                seen.insert(name.clone());
            }
        }
    };
    if let Some(h) = head {
        collect(h, &mut seen);
    }
    for lit in body {
        collect(literal_atom(lit), &mut seen);
    }
    seen.into_iter().collect()
}

/// Instantiate a rule/constraint over every assignment of its variables to the
/// universe, invoking `emit` with the substitution.
fn for_each_assignment(
    vars: &[String],
    universe: &[Val],
    mut emit: impl FnMut(&HashMap<String, Val>),
) -> Result<(), AspError> {
    let k = vars.len();
    // An empty universe grounds a rule with variables to nothing at all —
    // zero assignments, not an index panic (the `strata check`-ok /
    // `strata run`-panic hole an external review's follow-up found).
    if k > 0 && universe.is_empty() {
        return Ok(());
    }
    let u = universe.len().max(1);
    // universe^k, bounded.
    let total = (0..k)
        .try_fold(1usize, |acc, _| acc.checked_mul(u))
        .ok_or(AspError::TooLargeGrounding)?;
    if total > MAX_INSTANTIATIONS {
        return Err(AspError::TooLargeGrounding);
    }
    let mut idx = vec![0usize; k];
    loop {
        let asg: HashMap<String, Val> = vars
            .iter()
            .enumerate()
            .map(|(i, v)| (v.clone(), universe[idx[i]].clone()))
            .collect();
        emit(&asg);
        // increment mixed-radix counter
        let mut i = 0;
        while i < k {
            idx[i] += 1;
            if idx[i] < universe.len() {
                break;
            }
            idx[i] = 0;
            i += 1;
        }
        if i == k {
            break;
        }
    }
    Ok(())
}

fn ground_rule(
    r: &Rule,
    universe: &[Val],
    intern: &mut Interner,
    out: &mut Vec<GRule>,
) -> Result<(), AspError> {
    let vars = rule_vars(Some(&r.head), &r.body);
    let mut err = None;
    for_each_assignment(&vars, universe, |asg| {
        let head = match ground_atom(&r.head, asg) {
            Ok(a) => Some(intern.intern(a)),
            Err(e) => {
                err = Some(e);
                return;
            }
        };
        let (pos, neg) = match ground_body(&r.body, asg, intern) {
            Ok(pn) => pn,
            Err(e) => {
                err = Some(e);
                return;
            }
        };
        out.push(GRule { head, pos, neg });
    })?;
    err.map_or(Ok(()), Err)
}

fn ground_constraint(
    body: &[Literal],
    universe: &[Val],
    intern: &mut Interner,
    out: &mut Vec<GRule>,
) -> Result<(), AspError> {
    let vars = rule_vars(None, body);
    let mut err = None;
    for_each_assignment(&vars, universe, |asg| {
        match ground_body(body, asg, intern) {
            Ok((pos, neg)) => out.push(GRule {
                head: None,
                pos,
                neg,
            }),
            Err(e) => err = Some(e),
        }
    })?;
    err.map_or(Ok(()), Err)
}

fn ground_body(
    body: &[Literal],
    asg: &HashMap<String, Val>,
    intern: &mut Interner,
) -> Result<(Vec<usize>, Vec<usize>), AspError> {
    let mut pos = Vec::new();
    let mut neg = Vec::new();
    for lit in body {
        match lit {
            Literal::Pos(a) => pos.push(intern.intern(ground_atom(a, asg)?)),
            Literal::Neg(a) => neg.push(intern.intern(ground_atom(a, asg)?)),
        }
    }
    Ok((pos, neg))
}

fn ground_atom(a: &Atom, asg: &HashMap<String, Val>) -> Result<GroundAtom, AspError> {
    let mut args = Vec::with_capacity(a.args.len());
    for t in &a.args {
        args.push(match t {
            Term::Var { name } => asg
                .get(name)
                .cloned()
                .ok_or(AspError::Unsupported("unbound variable"))?,
            Term::Const { name } => Val::Sym(name.clone()),
            Term::Int { value } => Val::Int(*value),
            Term::Agg { .. } => return Err(AspError::Unsupported("aggregate atom")),
            Term::Compound { .. } => return Err(AspError::Unsupported("compound term")),
        });
    }
    Ok((a.pred.clone(), args))
}

fn literal_atom(lit: &Literal) -> &Atom {
    match lit {
        Literal::Pos(a) | Literal::Neg(a) => a,
    }
}

// --- the reference stable-model solver ---------------------------------------

/// Stable models of a set of ground rules, by guessing the negated atoms and
/// confirming the least model of the reduct reproduces the guess.
fn stable_models(n_atoms: usize, rules: &[GRule]) -> Result<Vec<BTreeSet<usize>>, AspError> {
    // N = the distinct atoms appearing negated (the only source of choice).
    let mut neg_atoms: Vec<usize> = rules
        .iter()
        .flat_map(|r| r.neg.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    neg_atoms.sort_unstable();
    if neg_atoms.len() > MAX_GUESS_ATOMS {
        return Err(AspError::TooManyChoices(neg_atoms.len()));
    }
    let n = neg_atoms.len();

    let mut found: BTreeSet<Vec<usize>> = BTreeSet::new();
    for mask in 0u32..(1u32 << n) {
        // The guessed-true negated atoms.
        let guess: HashSet<usize> = (0..n)
            .filter(|&i| mask & (1 << i) != 0)
            .map(|i| neg_atoms[i])
            .collect();

        // Reduct: keep head rules whose negatives are all guessed-false; drop negatives.
        let reduct: Vec<(usize, &[usize])> = rules
            .iter()
            .filter_map(|r| match r.head {
                Some(h) if r.neg.iter().all(|c| !guess.contains(c)) => Some((h, r.pos.as_slice())),
                _ => None,
            })
            .collect();
        let lm = least_model(n_atoms, &reduct);

        // Confirm the guess: LM restricted to N must equal the guessed-true set.
        let lm_neg: HashSet<usize> = neg_atoms
            .iter()
            .copied()
            .filter(|a| lm.contains(a))
            .collect();
        if lm_neg != guess {
            continue;
        }

        // Constraints must not fire (body satisfied under LM with negatives false).
        let violated = rules.iter().any(|r| {
            r.head.is_none()
                && r.neg.iter().all(|c| !guess.contains(c))
                && r.pos.iter().all(|p| lm.contains(p))
        });
        if violated {
            continue;
        }

        let mut sorted: Vec<usize> = lm.iter().copied().collect();
        sorted.sort_unstable();
        found.insert(sorted);
    }
    Ok(found.into_iter().map(|v| v.into_iter().collect()).collect())
}

/// Least model of a positive ground program (`(head, positive body)` rules).
fn least_model(n_atoms: usize, rules: &[(usize, &[usize])]) -> HashSet<usize> {
    let mut model: HashSet<usize> = HashSet::with_capacity(n_atoms);
    loop {
        let mut changed = false;
        for &(head, pos) in rules {
            if !model.contains(&head) && pos.iter().all(|p| model.contains(p)) {
                model.insert(head);
                changed = true;
            }
        }
        if !changed {
            return model;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_universe_grounds_a_var_rule_to_nothing() {
        // No constants anywhere: a rule with variables has zero instantiations
        // (never an index panic), and the empty model is the one stable model.
        use strata_ir::high::program::{atom, var, Literal, Rule};
        let rules = vec![Rule {
            head: atom("p", vec![var("X")]),
            body: vec![Literal::Neg(atom("q", vec![var("X")]))],
        }];
        let models = solve(&rules, &[], &[]).expect("solve");
        assert_eq!(models.len(), 1);
        assert!(models[0].is_empty(), "the empty model is stable");
    }
    use strata_ir::high::program::{atom, var, Rule};

    fn a(pred: &str) -> Atom {
        atom(pred, vec![])
    }
    fn rule(head: Atom, body: Vec<Literal>) -> Rule {
        Rule { head, body }
    }
    fn models(
        rules: &[Rule],
        facts: &[GroundAtom],
        constraints: &[Vec<Literal>],
    ) -> Vec<Vec<GroundAtom>> {
        solve(rules, facts, constraints).unwrap()
    }

    #[test]
    fn even_cycle_has_two_stable_models() {
        // a :- not b.  b :- not a.  ⇒ {a} and {b}.
        let rules = vec![
            rule(a("a"), vec![Literal::Neg(a("b"))]),
            rule(a("b"), vec![Literal::Neg(a("a"))]),
        ];
        let m = models(&rules, &[], &[]);
        assert_eq!(
            m,
            vec![vec![("a".into(), vec![])], vec![("b".into(), vec![])]]
        );
    }

    #[test]
    fn odd_cycle_has_no_stable_model() {
        // p :- not p.  ⇒ no answer set.
        let rules = vec![rule(a("p"), vec![Literal::Neg(a("p"))])];
        assert!(models(&rules, &[], &[]).is_empty());
    }

    #[test]
    fn constraint_filters_a_model() {
        // a :- not b.  b :- not a.  :- a.  ⇒ only {b}.
        let rules = vec![
            rule(a("a"), vec![Literal::Neg(a("b"))]),
            rule(a("b"), vec![Literal::Neg(a("a"))]),
        ];
        let constraints = vec![vec![Literal::Pos(a("a"))]];
        let m = models(&rules, &[], &constraints);
        assert_eq!(m, vec![vec![("b".into(), vec![])]]);
    }

    #[test]
    fn first_order_choice_over_a_domain() {
        // node(a). node(b).  in(X):-node(X),not out(X).  out(X):-node(X),not in(X).
        // Each node is independently in/out ⇒ 4 stable models.
        let facts = vec![
            ("node".to_string(), vec![Val::Sym("a".into())]),
            ("node".to_string(), vec![Val::Sym("b".into())]),
        ];
        let node = |v: &str| atom("node", vec![var(v)]);
        let inn = |v: &str| atom("in", vec![var(v)]);
        let out = |v: &str| atom("out", vec![var(v)]);
        let rules = vec![
            rule(
                inn("X"),
                vec![Literal::Pos(node("X")), Literal::Neg(out("X"))],
            ),
            rule(
                out("X"),
                vec![Literal::Pos(node("X")), Literal::Neg(inn("X"))],
            ),
        ];
        let m = models(&rules, &facts, &[]);
        assert_eq!(m.len(), 4, "each of 2 nodes independently in/out");
        // every model keeps both node facts
        assert!(m
            .iter()
            .all(|s| s.contains(&("node".into(), vec![Val::Sym("a".into())]))));
    }
}
