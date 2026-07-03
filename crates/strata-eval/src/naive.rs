//! Naive immediate-consequence operator `T_P` + stratified driver. [EVAL-3/4/5/6/7, D7]
//!
//! Repeatedly evaluate every rule body against the CURRENT full relations,
//! materialize head tuples with combined annotations, apply, repeat until no
//! relation changes (spec 1.1 least fixpoint). Bool via set union; Trop via
//! min-plus. The outer driver saturates strata in order (0..num_strata), freezing
//! lower strata before the next (EVAL-5, perfect model).
//!
//! - EVAL-4: `not B` is evaluated against strictly-lower, already-saturated strata
//!   (asserted defensively).
//! - EVAL-6: aggregate heads `H(X, agg⟨Y⟩) :- B` are evaluated once per stratum
//!   (bodies must live in strictly lower strata — non-recursive, spec 1.3).
//! - EVAL-7: Trop `⊗` overflow is a runtime error (D6); negative-weight cycles
//!   are detected by a Bellman-Ford iteration bound and reported, never looped.

use std::collections::{HashMap, HashSet};

use strata_ir::core::{CoreAtom, CoreLiteral, CoreProgram, CoreRule, CoreTerm, Semiring};
use strata_ir::high::program::AggOp;
use strata_ir::trop::TropOverflow;

use crate::store::{Db, Tuple};
use crate::value::{Ann, GroundVal};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    /// A body/head predicate has no relation (undeclared). CHECK-2 catches this
    /// statically; the oracle still fails loud rather than miscompute.
    UnknownPred(String),
    /// Trop `⊗` overflowed i64 (D6): a runtime error, never a wrap.
    Overflow(TropOverflow),
    /// A head variable slot was never bound by the body (range-restriction /
    /// safety; CHECK-13). The oracle refuses to invent a value.
    UnsafeHeadVar(u32),
    /// A negated literal's variable was unbound when reached (unsafe negation /
    /// body ordering; CHECK-10/13).
    UnsafeNegVar(u32),
    /// A negated predicate is not in a strictly lower stratum than its rule
    /// (stratification invariant; CHECK-3). The evaluator refuses to proceed.
    NegationNotStratified { pred: String },
    /// A tropical stratum failed to converge within the Bellman-Ford bound —
    /// a negative-weight cycle (spec 2.4). [EVAL-7]
    NegativeWeightCycle,
    /// An aggregate rule's body predicate is not strictly lower (agg-in-recursion
    /// is forbidden, spec 1.3; CHECK-9). The oracle fails loud.
    AggregateInRecursion { pred: String },
    /// An aggregate head is malformed (not exactly one aggregate term).
    MalformedAggregateHead,
    /// `sum`/`min`/`max` require an integer aggregand.
    NonIntegerAggregand,
    /// A construct outside Phase-0 eval scope reached the interpreter.
    Unsupported(&'static str),
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use EvalError::*;
        match self {
            UnknownPred(p) => write!(f, "unknown predicate {p:?}"),
            Overflow(o) => write!(f, "{o}"),
            UnsafeHeadVar(s) => write!(f, "unbound head variable in slot {s}"),
            UnsafeNegVar(s) => write!(f, "unbound variable in negated literal, slot {s}"),
            NegationNotStratified { pred } => {
                write!(
                    f,
                    "negated predicate {pred:?} is not in a strictly lower stratum"
                )
            }
            NegativeWeightCycle => write!(f, "negative-weight cycle: tropical stratum diverges"),
            AggregateInRecursion { pred } => {
                write!(
                    f,
                    "aggregate body predicate {pred:?} is not in a strictly lower stratum"
                )
            }
            MalformedAggregateHead => write!(f, "aggregate head must contain exactly one agg term"),
            NonIntegerAggregand => write!(f, "min/max/sum require an integer aggregand"),
            Unsupported(w) => write!(f, "not supported in Phase-0 eval: {w}"),
        }
    }
}

impl std::error::Error for EvalError {}

/// Evaluation context threaded through the join (avoids long argument lists).
struct Ctx<'a> {
    db: &'a Db,
    pred_stratum: &'a HashMap<String, u32>,
    cur_stratum: u32,
}

/// Run the program to its least fixpoint over `edb`, returning the saturated
/// database. `edb` is seeded into a fresh [`Db`] built from the program's
/// predicate declarations.
pub fn run(prog: &CoreProgram, edb: &[(&str, Tuple, Ann)]) -> Result<Db, EvalError> {
    let mut db = Db::from_program(prog);
    for (pred, tuple, ann) in edb {
        db.relation_mut(pred)
            .ok_or_else(|| EvalError::UnknownPred((*pred).to_string()))?;
        db.insert(pred, tuple.clone(), *ann);
    }

    let pred_stratum: HashMap<String, u32> = prog
        .predicates
        .iter()
        .map(|p| (p.name.clone(), p.stratum))
        .collect();
    let pred_sem: HashMap<String, Semiring> = prog
        .predicates
        .iter()
        .map(|p| (p.name.clone(), p.semiring))
        .collect();
    // The Herbrand universe is fixed by the EDB (no value invention without terms),
    // so the count of distinct constants bounds Bellman-Ford convergence (EVAL-7).
    let universe = distinct_symbols(&db);

    for k in 0..prog.num_strata {
        let is_trop = prog
            .rules_in_stratum(k)
            .any(|r| pred_sem.get(&r.head.pred) == Some(&Semiring::Trop));
        saturate_stratum(prog, &mut db, k, &pred_stratum, is_trop, universe)?;
    }
    Ok(db)
}

pub(crate) fn distinct_symbols(db: &Db) -> usize {
    let mut set = HashSet::new();
    for pred in db.predicates() {
        for tuple in db.relation(pred).unwrap().rows.keys() {
            for v in tuple {
                if let GroundVal::Sym(s) = v {
                    set.insert(*s);
                }
            }
        }
    }
    set.len()
}

pub(crate) fn is_aggregate_rule(rule: &CoreRule) -> bool {
    rule.head
        .args
        .iter()
        .any(|a| matches!(a, CoreTerm::Agg { .. }))
}

/// Naive `T_P` to a fixpoint over one stratum, with lower strata frozen.
fn saturate_stratum(
    prog: &CoreProgram,
    db: &mut Db,
    stratum: u32,
    pred_stratum: &HashMap<String, u32>,
    is_trop: bool,
    universe: usize,
) -> Result<(), EvalError> {
    // Aggregate rules are non-recursive (bodies in lower strata): evaluate once.
    for rule in prog
        .rules_in_stratum(stratum)
        .filter(|r| is_aggregate_rule(r))
    {
        eval_aggregate_rule(db, rule, pred_stratum, stratum)?;
    }

    // Bellman-Ford bound: min-plus SSSP converges within `universe` relaxation
    // rounds; a change past that implies a negative-weight cycle (EVAL-7).
    let cap = universe + 2;
    let mut iters = 0usize;
    loop {
        let mut derived: Vec<(String, Tuple, Ann)> = Vec::new();
        for rule in prog
            .rules_in_stratum(stratum)
            .filter(|r| !is_aggregate_rule(r))
        {
            let head_sem = db
                .relation(&rule.head.pred)
                .ok_or_else(|| EvalError::UnknownPred(rule.head.pred.clone()))?
                .semiring;
            let ctx = Ctx {
                db,
                pred_stratum,
                cur_stratum: stratum,
            };
            let mut solved: Vec<(Vec<Option<GroundVal>>, Ann)> = Vec::new();
            let mut binding = vec![None; rule.var_count as usize];
            solve(
                &ctx,
                &rule.body,
                0,
                &mut binding,
                Ann::otimes_id(head_sem),
                &mut solved,
            )?;
            for (b, acc) in solved {
                derived.push((rule.head.pred.clone(), build_head(&rule.head, &b)?, acc));
            }
        }
        let mut changed = false;
        for (pred, tuple, ann) in derived {
            let rel = db.relation_mut(&pred).ok_or(EvalError::UnknownPred(pred))?;
            changed |= rel.combine(tuple, ann);
        }
        iters += 1;
        if !changed {
            return Ok(());
        }
        if is_trop && iters > cap {
            return Err(EvalError::NegativeWeightCycle);
        }
    }
}

/// Solve a rule body, pushing each completed `(binding, product-annotation)`.
fn solve(
    ctx: &Ctx,
    body: &[CoreLiteral],
    idx: usize,
    binding: &mut [Option<GroundVal>],
    acc: Ann,
    out: &mut Vec<(Vec<Option<GroundVal>>, Ann)>,
) -> Result<(), EvalError> {
    if idx == body.len() {
        out.push((binding.to_vec(), acc));
        return Ok(());
    }
    match &body[idx] {
        CoreLiteral::Pos(atom) => {
            let rel = ctx
                .db
                .relation(&atom.pred)
                .ok_or_else(|| EvalError::UnknownPred(atom.pred.clone()))?;
            for (tuple, &row_ann) in &rel.rows {
                let mut b = binding.to_vec();
                if unify(&atom.args, tuple, &mut b) {
                    let new_acc = acc.otimes(row_ann).map_err(EvalError::Overflow)?;
                    solve(ctx, body, idx + 1, &mut b, new_acc, out)?;
                }
            }
            Ok(())
        }
        CoreLiteral::Neg(atom) => {
            // Defensive stratification (EVAL-4): negated pred strictly lower.
            let ns = ctx
                .pred_stratum
                .get(&atom.pred)
                .ok_or_else(|| EvalError::UnknownPred(atom.pred.clone()))?;
            if *ns >= ctx.cur_stratum {
                return Err(EvalError::NegationNotStratified {
                    pred: atom.pred.clone(),
                });
            }
            let tuple = ground_atom(atom, binding, true)?;
            let rel = ctx
                .db
                .relation(&atom.pred)
                .ok_or_else(|| EvalError::UnknownPred(atom.pred.clone()))?;
            if rel.rows.contains_key(&tuple) {
                // negation fails for this binding → prune this derivation branch
                Ok(())
            } else {
                // negation holds; `⊗` with Bool identity leaves acc unchanged
                solve(ctx, body, idx + 1, binding, acc, out)
            }
        }
    }
}

/// Aggregate rule `H(X, agg⟨Y⟩) :- B` (spec 1.3), evaluated once. [EVAL-6]
pub(crate) fn eval_aggregate_rule(
    db: &mut Db,
    rule: &CoreRule,
    pred_stratum: &HashMap<String, u32>,
    cur: u32,
) -> Result<(), EvalError> {
    // Defensive: aggregate bodies must be strictly lower (non-recursive; CHECK-9).
    for lit in &rule.body {
        let (CoreLiteral::Pos(a) | CoreLiteral::Neg(a)) = lit;
        let s = pred_stratum
            .get(&a.pred)
            .ok_or_else(|| EvalError::UnknownPred(a.pred.clone()))?;
        if *s >= cur {
            return Err(EvalError::AggregateInRecursion {
                pred: a.pred.clone(),
            });
        }
    }

    // Locate the single aggregate head term.
    let mut agg_pos = None;
    for (i, arg) in rule.head.args.iter().enumerate() {
        if let CoreTerm::Agg { op, slot } = arg {
            if agg_pos.is_some() {
                return Err(EvalError::MalformedAggregateHead);
            }
            agg_pos = Some((i, *op, *slot));
        }
    }
    let (agg_index, op, agg_slot) = agg_pos.ok_or(EvalError::MalformedAggregateHead)?;

    // Collect every body binding.
    let ctx = Ctx {
        db,
        pred_stratum,
        cur_stratum: cur,
    };
    let mut solved = Vec::new();
    let mut binding = vec![None; rule.var_count as usize];
    solve(&ctx, &rule.body, 0, &mut binding, Ann::Unit, &mut solved)?;

    // Group by the non-aggregate head args; fold the aggregand.
    let mut groups: HashMap<Vec<GroundVal>, Vec<GroundVal>> = HashMap::new();
    for (b, _) in &solved {
        let mut key = Vec::new();
        for (i, arg) in rule.head.args.iter().enumerate() {
            if i == agg_index {
                continue;
            }
            key.push(head_arg_value(arg, b)?);
        }
        let aggregand = b[agg_slot as usize].ok_or(EvalError::UnsafeHeadVar(agg_slot))?;
        groups.entry(key).or_default().push(aggregand);
    }

    for (key, vals) in groups {
        let result = fold_aggregate(op, &vals)?;
        let mut tuple = Vec::with_capacity(rule.head.args.len());
        let mut ki = 0;
        for (i, _) in rule.head.args.iter().enumerate() {
            if i == agg_index {
                tuple.push(GroundVal::Int(result));
            } else {
                tuple.push(key[ki]);
                ki += 1;
            }
        }
        db.insert(&rule.head.pred, tuple, Ann::Unit);
    }
    Ok(())
}

fn fold_aggregate(op: AggOp, vals: &[GroundVal]) -> Result<i64, EvalError> {
    let ints = || -> Result<Vec<i64>, EvalError> {
        vals.iter()
            .map(|v| match v {
                GroundVal::Int(n) => Ok(*n),
                GroundVal::Sym(_) => Err(EvalError::NonIntegerAggregand),
            })
            .collect()
    };
    match op {
        AggOp::Count => Ok(vals.len() as i64),
        AggOp::Sum => Ok(ints()?.iter().sum()),
        AggOp::Min => ints()?
            .into_iter()
            .min()
            .ok_or(EvalError::NonIntegerAggregand),
        AggOp::Max => ints()?
            .into_iter()
            .max()
            .ok_or(EvalError::NonIntegerAggregand),
        AggOp::ProbOr => Err(EvalError::Unsupported("prob-or aggregate (режим B)")),
    }
}

/// Unify an atom's terms against a ground tuple under `binding`.
pub(crate) fn unify(
    args: &[CoreTerm],
    tuple: &[GroundVal],
    binding: &mut [Option<GroundVal>],
) -> bool {
    if args.len() != tuple.len() {
        return false;
    }
    for (arg, &val) in args.iter().zip(tuple) {
        match arg {
            CoreTerm::Var { slot } => match binding[*slot as usize] {
                Some(bound) if bound != val => return false,
                Some(_) => {}
                None => binding[*slot as usize] = Some(val),
            },
            CoreTerm::Const { sym } => {
                if val != GroundVal::Sym(*sym) {
                    return false;
                }
            }
            CoreTerm::Int { value } => {
                if val != GroundVal::Int(*value) {
                    return false;
                }
            }
            CoreTerm::Agg { .. } => return false, // aggregates never appear in bodies
        }
    }
    true
}

/// Build a ground tuple from an atom under a (complete) binding. `negated`
/// selects the appropriate "unbound variable" error variant.
fn ground_atom(
    atom: &CoreAtom,
    binding: &[Option<GroundVal>],
    negated: bool,
) -> Result<Tuple, EvalError> {
    let mut tuple = Vec::with_capacity(atom.args.len());
    for arg in &atom.args {
        tuple.push(match arg {
            CoreTerm::Var { slot } => binding[*slot as usize].ok_or(if negated {
                EvalError::UnsafeNegVar(*slot)
            } else {
                EvalError::UnsafeHeadVar(*slot)
            })?,
            CoreTerm::Const { sym } => GroundVal::Sym(*sym),
            CoreTerm::Int { value } => GroundVal::Int(*value),
            CoreTerm::Agg { .. } => {
                return Err(EvalError::Unsupported("aggregate in atom position"))
            }
        });
    }
    Ok(tuple)
}

fn head_arg_value(arg: &CoreTerm, binding: &[Option<GroundVal>]) -> Result<GroundVal, EvalError> {
    match arg {
        CoreTerm::Var { slot } => binding[*slot as usize].ok_or(EvalError::UnsafeHeadVar(*slot)),
        CoreTerm::Const { sym } => Ok(GroundVal::Sym(*sym)),
        CoreTerm::Int { value } => Ok(GroundVal::Int(*value)),
        CoreTerm::Agg { .. } => Err(EvalError::MalformedAggregateHead),
    }
}

pub(crate) fn build_head(
    head: &CoreAtom,
    binding: &[Option<GroundVal>],
) -> Result<Tuple, EvalError> {
    ground_atom(head, binding, false)
}

/// Negation check shared with semi-naive: does `atom` (all vars bound) hold as a
/// stratified negation at `cur_stratum`? Returns `Ok(true)` if the negation is
/// satisfied (tuple absent) so the derivation continues.
pub(crate) fn negation_holds(
    atom: &CoreAtom,
    binding: &[Option<GroundVal>],
    db: &Db,
    pred_stratum: &HashMap<String, u32>,
    cur_stratum: u32,
) -> Result<bool, EvalError> {
    let ns = pred_stratum
        .get(&atom.pred)
        .ok_or_else(|| EvalError::UnknownPred(atom.pred.clone()))?;
    if *ns >= cur_stratum {
        return Err(EvalError::NegationNotStratified {
            pred: atom.pred.clone(),
        });
    }
    let tuple = ground_atom(atom, binding, true)?;
    let rel = db
        .relation(&atom.pred)
        .ok_or_else(|| EvalError::UnknownPred(atom.pred.clone()))?;
    Ok(!rel.rows.contains_key(&tuple))
}
