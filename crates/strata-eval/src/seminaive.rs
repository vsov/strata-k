//! Semi-naive (delta) evaluation. [EVAL-8, D7, spec 3.2 conceptually]
//!
//! Each recursive round joins using at least one *delta* tuple (a fact new or
//! improved in the previous round), so already-derived tuples are not
//! recomputed — the main defect of naive iteration. Aggregates and negation
//! touch only frozen lower strata, so they reuse the naive paths unchanged.
//!
//! Correctness is asserted, not assumed: `run_semi_naive` must produce a database
//! bit-identical to naive [`crate::naive::run`] on every program. The in-crate
//! cross-check (tests/interp.rs) is the core Phase-0 correctness signal (I5), and
//! validates the algorithm before it is ported to the GPU.

use std::collections::{HashMap, HashSet};

use strata_ir::core::{CoreLiteral, CoreProgram, Semiring};
use strata_ir::terms::TermTable;

use crate::naive::{
    build_head, distinct_symbols, eval_aggregate_rule, is_aggregate_rule, negation_holds, unify,
    EvalError,
};
use crate::store::{Db, Tuple};
use crate::value::{Ann, GroundVal};

/// Delta rows per predicate: tuples that changed in the previous round.
type Delta = HashMap<String, Vec<(Tuple, Ann)>>;

/// Run the program to its least fixpoint with semi-naive evaluation.
pub fn run_semi_naive(prog: &CoreProgram, edb: &[(&str, Tuple, Ann)]) -> Result<Db, EvalError> {
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
    let universe = distinct_symbols(&db);
    // Semi-naive is the non-`@terms` path (the CLI routes `@terms` to naive), so
    // this table is a throwaway that the helpers thread but never populate.
    let mut terms = TermTable::new(0);

    for k in 0..prog.num_strata {
        let is_trop = prog
            .rules_in_stratum(k)
            .any(|r| pred_sem.get(&r.head.pred) == Some(&Semiring::Trop));
        saturate_stratum(
            prog,
            &mut db,
            k,
            &pred_stratum,
            is_trop,
            universe,
            &mut terms,
        )?;
    }
    Ok(db)
}

#[allow(clippy::too_many_arguments)]
fn saturate_stratum(
    prog: &CoreProgram,
    db: &mut Db,
    stratum: u32,
    pred_stratum: &HashMap<String, u32>,
    is_trop: bool,
    universe: usize,
    terms: &mut TermTable,
) -> Result<(), EvalError> {
    for rule in prog
        .rules_in_stratum(stratum)
        .filter(|r| is_aggregate_rule(r))
    {
        eval_aggregate_rule(db, rule, pred_stratum, stratum, terms)?;
    }

    // IDB predicates of this stratum: the ones that grow here and carry deltas.
    let idb: HashSet<String> = prog
        .rules_in_stratum(stratum)
        .filter(|r| !is_aggregate_rule(r))
        .map(|r| r.head.pred.clone())
        .collect();

    // Bootstrap: one full naive step seeds the initial delta.
    let mut delta = apply_round(prog, db, stratum, pred_stratum, None, terms)?;

    let cap = universe + 2;
    let mut iters = 0usize;
    while delta.values().any(|v| !v.is_empty()) {
        delta = apply_round(prog, db, stratum, pred_stratum, Some((&idb, &delta)), terms)?;
        iters += 1;
        if is_trop && iters > cap {
            return Err(EvalError::NegativeWeightCycle);
        }
    }
    Ok(())
}

/// Evaluate one round and apply it, returning the tuples that changed.
///
/// `mode = None` is the bootstrap full step. `mode = Some((idb, delta))` is a
/// delta round: each rule is evaluated once per recursive body position that has
/// a non-empty delta, with that position ranging over the delta.
#[allow(clippy::too_many_arguments)]
fn apply_round(
    prog: &CoreProgram,
    db: &mut Db,
    stratum: u32,
    pred_stratum: &HashMap<String, u32>,
    mode: Option<(&HashSet<String>, &Delta)>,
    terms: &mut TermTable,
) -> Result<Delta, EvalError> {
    let mut derived: Vec<(String, Tuple, Ann)> = Vec::new();

    for rule in prog
        .rules_in_stratum(stratum)
        .filter(|r| !is_aggregate_rule(r))
    {
        let head_sem = db
            .relation(&rule.head.pred)
            .ok_or_else(|| EvalError::UnknownPred(rule.head.pred.clone()))?
            .semiring;
        let id = Ann::otimes_id(head_sem);

        match mode {
            None => {
                let mut solved = Vec::new();
                solve(
                    db,
                    terms,
                    &rule.body,
                    0,
                    &mut vec![None; rule.var_count as usize],
                    id,
                    None,
                    pred_stratum,
                    stratum,
                    &mut solved,
                )?;
                for (b, acc) in solved {
                    if let Some(t) = build_head(&rule.head, &b, terms)? {
                        derived.push((rule.head.pred.clone(), t, acc));
                    }
                }
            }
            Some((idb, delta)) => {
                for (i, lit) in rule.body.iter().enumerate() {
                    let CoreLiteral::Pos(atom) = lit else {
                        continue;
                    };
                    if !idb.contains(&atom.pred) {
                        continue;
                    }
                    let Some(rows) = delta.get(&atom.pred) else {
                        continue;
                    };
                    if rows.is_empty() {
                        continue;
                    }
                    let mut solved = Vec::new();
                    solve(
                        db,
                        terms,
                        &rule.body,
                        0,
                        &mut vec![None; rule.var_count as usize],
                        id,
                        Some((i, rows)),
                        pred_stratum,
                        stratum,
                        &mut solved,
                    )?;
                    for (b, acc) in solved {
                        if let Some(t) = build_head(&rule.head, &b, terms)? {
                            derived.push((rule.head.pred.clone(), t, acc));
                        }
                    }
                }
            }
        }
    }

    // Apply, tracking which (pred, tuple) changed.
    let mut changed_keys: HashSet<(String, Tuple)> = HashSet::new();
    for (pred, tuple, ann) in derived {
        let rel = db
            .relation_mut(&pred)
            .ok_or_else(|| EvalError::UnknownPred(pred.clone()))?;
        if rel.combine(tuple.clone(), ann) {
            changed_keys.insert((pred, tuple));
        }
    }

    // Next delta = the merged (current) value of each changed tuple.
    let mut next: Delta = HashMap::new();
    for (pred, tuple) in changed_keys {
        let ann = *db.relation(&pred).unwrap().rows.get(&tuple).unwrap();
        next.entry(pred).or_default().push((tuple, ann));
    }
    Ok(next)
}

/// Solve a rule body; at `delta_pos` (if given) the literal ranges over `delta`
/// rows instead of the full relation. Otherwise identical to the naive join.
#[allow(clippy::too_many_arguments)]
fn solve(
    db: &Db,
    terms: &mut TermTable,
    body: &[CoreLiteral],
    idx: usize,
    binding: &mut [Option<GroundVal>],
    acc: Ann,
    delta: Option<(usize, &[(Tuple, Ann)])>,
    pred_stratum: &HashMap<String, u32>,
    cur_stratum: u32,
    out: &mut Vec<(Vec<Option<GroundVal>>, Ann)>,
) -> Result<(), EvalError> {
    if idx == body.len() {
        out.push((binding.to_vec(), acc));
        return Ok(());
    }
    match &body[idx] {
        CoreLiteral::Pos(atom) => {
            let use_delta = matches!(delta, Some((p, _)) if p == idx);
            if use_delta {
                let (_, rows) = delta.unwrap();
                for (tuple, row_ann) in rows {
                    let mut b = binding.to_vec();
                    if unify(&atom.args, tuple, &mut b, terms) {
                        let new_acc = acc.otimes(*row_ann).map_err(EvalError::Overflow)?;
                        solve(
                            db,
                            terms,
                            body,
                            idx + 1,
                            &mut b,
                            new_acc,
                            delta,
                            pred_stratum,
                            cur_stratum,
                            out,
                        )?;
                    }
                }
            } else {
                let rel = db
                    .relation(&atom.pred)
                    .ok_or_else(|| EvalError::UnknownPred(atom.pred.clone()))?;
                for (tuple, &row_ann) in &rel.rows {
                    let mut b = binding.to_vec();
                    if unify(&atom.args, tuple, &mut b, terms) {
                        let new_acc = acc.otimes(row_ann).map_err(EvalError::Overflow)?;
                        solve(
                            db,
                            terms,
                            body,
                            idx + 1,
                            &mut b,
                            new_acc,
                            delta,
                            pred_stratum,
                            cur_stratum,
                            out,
                        )?;
                    }
                }
            }
            Ok(())
        }
        CoreLiteral::Neg(atom) => {
            if negation_holds(atom, binding, db, pred_stratum, cur_stratum, terms)? {
                solve(
                    db,
                    terms,
                    body,
                    idx + 1,
                    binding,
                    acc,
                    delta,
                    pred_stratum,
                    cur_stratum,
                    out,
                )
            } else {
                Ok(())
            }
        }
    }
}
