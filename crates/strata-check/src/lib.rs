//! `strata-check` — static analysis and normalization High-IR → Core-IR.
//! [CHECK-2/3/4/10/12/13, D4, D5]
//!
//! `check_program` runs the pipeline (declarations → dependency graph →
//! stratification → range-restriction/semiring checks → lowering) and returns a
//! [`Checked`] bundle (Core-IR + symbol dictionary + ground EDB) or the
//! accumulated diagnostics. Depends only on strata-ir (D14); emits the shared
//! [`Diagnostics`] (IR-9) in the E1xxx range.

pub mod diagnostics;

use std::collections::HashMap;

use strata_ir::core::{CoreAtom, CoreLiteral, CorePred, CoreProgram, CoreRule, CoreTerm, Semiring};
use strata_ir::diag::Span;
use strata_ir::dict::SymbolDict;
use strata_ir::high::program::{Atom, ItemKind, Literal, Program, QueryKind, Term};
use strata_ir::high::sig::Annotation;
use strata_ir::value::{GroundFact, GroundVal};

pub use diagnostics::{codes, Diagnostics};

/// A resolved query: `?[prob|grad] pred(pattern)`, `None` in a pattern position
/// means a variable (any value).
#[derive(Debug, Clone, PartialEq)]
pub struct QuerySpec {
    pub kind: QueryKind,
    pub pred: String,
    pub pattern: Vec<Option<GroundVal>>,
}

/// The result of checking: everything the interpreter needs to run.
#[derive(Debug, Clone)]
pub struct Checked {
    pub core: CoreProgram,
    pub dict: SymbolDict,
    /// Certain (Bool/Trop) ground facts.
    pub edb: Vec<GroundFact>,
    /// Probabilistic facts `p :: atom` (режим B, Phase 4).
    pub prob_edb: Vec<(String, Vec<GroundVal>, f64)>,
    /// Queries in source order.
    pub queries: Vec<QuerySpec>,
}

#[derive(Debug, Clone, Copy)]
struct PredInfo {
    arity: u32,
    annotation: Annotation,
}

impl PredInfo {
    /// The executable semiring, or `None` for Prov/Prov_k (режим B, out of Phase 0).
    fn semiring(&self) -> Option<Semiring> {
        annotation_semiring(&self.annotation)
    }
}

const NOSPAN: Span = Span { start: 0, end: 0 };

fn annotation_semiring(a: &Annotation) -> Option<Semiring> {
    match a {
        Annotation::Bool => Some(Semiring::Bool),
        Annotation::Trop => Some(Semiring::Trop),
        Annotation::Prov | Annotation::ProvK { .. } => None,
    }
}

/// Bool ⊑ Trop is the only executable coercion; Trop cannot flow into Bool.
fn coercible(from: Semiring, to: Semiring) -> bool {
    matches!(
        (from, to),
        (Semiring::Bool, _) | (Semiring::Trop, Semiring::Trop)
    )
}

/// Check and lower a High-IR program to Core-IR.
pub fn check_program(program: &Program) -> Result<Checked, Diagnostics> {
    let mut diags = Diagnostics::new();

    // 1. Collect declarations (with their clause spans for diagnostics).
    let mut declared: HashMap<String, PredInfo> = HashMap::new();
    let mut declared_span: HashMap<String, Span> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for item in &program.items {
        if let ItemKind::Predicate(p) = &item.node {
            if !declared.contains_key(&p.name) {
                order.push(p.name.clone());
            }
            declared.insert(
                p.name.clone(),
                PredInfo {
                    arity: p.sig.args.len() as u32,
                    annotation: p.sig.annotation,
                },
            );
            declared_span.insert(p.name.clone(), item.span);
        }
    }

    // 2. Undeclared / arity checks over used atoms.
    let mut reported_undeclared: std::collections::HashSet<String> = Default::default();
    let mut check_atom =
        |atom: &Atom, span: Span, diags: &mut Diagnostics| match declared.get(&atom.pred) {
            None => {
                if reported_undeclared.insert(atom.pred.clone()) {
                    diags.error(
                        codes::UNDECLARED_PRED,
                        format!("predicate `{}` is used but never declared", atom.pred),
                        span,
                    );
                }
            }
            // Executability (Prov/Prov_k) is decided per-predicate by the
            // table-2.4 pass below, not per-use.
            Some(info) => {
                if info.arity as usize != atom.args.len() {
                    diags.error(
                        codes::ARITY_MISMATCH,
                        format!(
                            "predicate `{}` expects {} argument(s), found {}",
                            atom.pred,
                            info.arity,
                            atom.args.len()
                        ),
                        span,
                    );
                }
            }
        };
    for item in &program.items {
        match &item.node {
            ItemKind::Rule(r) => {
                check_atom(&r.head, item.span, &mut diags);
                for lit in &r.body {
                    check_atom(literal_atom(lit), item.span, &mut diags);
                }
            }
            ItemKind::Fact(f) => check_atom(&f.atom, item.span, &mut diags),
            ItemKind::Query(q) => check_atom(&q.atom, item.span, &mut diags),
            _ => {}
        }
    }
    if diags.has_errors() {
        return Err(diags);
    }

    // 3. Dependency graph over declared predicates.
    let idx: HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let mut edges: Vec<(usize, usize, bool)> = Vec::new();
    for item in &program.items {
        if let ItemKind::Rule(r) = &item.node {
            let h = idx[r.head.pred.as_str()];
            let is_agg = r.head.args.iter().any(|t| matches!(t, Term::Agg { .. }));
            for lit in &r.body {
                let (atom, negated) = match lit {
                    Literal::Pos(a) => (a, false),
                    Literal::Neg(a) => (a, true),
                };
                let b = idx[atom.pred.as_str()];
                edges.push((h, b, negated || is_agg));
            }
        }
    }

    // 4. Stratification.
    let (stratum, num_strata) = match stratify(order.len(), &edges) {
        Ok(s) => s,
        Err(bad) => {
            diags.error(
                codes::UNSTRATIFIABLE,
                format!(
                    "predicate `{}` depends on its own negation/aggregation through a cycle",
                    order[bad]
                ),
                declared_span.get(&order[bad]).copied().unwrap_or(NOSPAN),
            );
            return Err(diags);
        }
    };

    // 4b. Table 2.4: classify each predicate by (annotation × recursion) and
    // reject forbidden cells with the nearest allowed alternative (spec 2.4, D9).
    let recursive = recursive_preds(order.len(), &edges);
    for (i, name) in order.iter().enumerate() {
        let span = declared_span.get(name).copied().unwrap_or(NOSPAN);
        let rec = recursive.contains(&i);
        match declared[name].annotation {
            // режим A, always allowed (recursive Trop gets negative-cycle
            // detection at eval, EVAL-7).
            Annotation::Bool | Annotation::Trop => {}
            // Exact probabilistic provenance through recursion is forbidden
            // (formal power series / #P). Nearest allowed: Prov_k (top-k).
            Annotation::Prov if rec => diags.error(
                codes::TABLE_2_4_FORBIDDEN,
                format!(
                    "recursive probabilistic provenance `{name}` is forbidden exactly; \
                     annotate it `Prov_k` for a top-k lower bound"
                ),
                span,
            ),
            // Non-recursive Prov and Prov_k are режим B (SDD/WMC), not executed
            // in Phase 0 (D5).
            Annotation::Prov => diags.error(
                codes::NOT_EXECUTABLE,
                format!(
                    "probabilistic provenance `{name}` needs режим B (SDD/WMC), \
                     not implemented in Phase 0"
                ),
                span,
            ),
            Annotation::ProvK { .. } => diags.error(
                codes::NOT_EXECUTABLE,
                format!("`Prov_k` provenance `{name}` is not implemented in Phase 0"),
                span,
            ),
        }
    }
    if diags.has_errors() {
        return Err(diags);
    }

    // 5. Range-restriction, semiring consistency, and lowering.
    let mut dict = SymbolDict::new();
    let mut rules: Vec<CoreRule> = Vec::new();
    let mut edb: Vec<GroundFact> = Vec::new();
    let mut prob_edb: Vec<(String, Vec<GroundVal>, f64)> = Vec::new();
    let mut queries: Vec<QuerySpec> = Vec::new();

    for item in &program.items {
        match &item.node {
            ItemKind::Rule(r) => {
                check_range_restriction(r, item.span, &mut diags);
                check_semirings(r, item.span, &declared, &mut diags);
                if let Some(rule) = lower_rule(r, &declared, &stratum, &idx, &mut dict) {
                    rules.push(rule);
                }
            }
            ItemKind::Fact(f) => {
                if let Some(args) = ground_fact_args(f, item.span, &mut dict, &mut diags) {
                    match f.prob {
                        Some(p) => prob_edb.push((f.atom.pred.clone(), args, p)),
                        None => edb.push(GroundFact {
                            pred: f.atom.pred.clone(),
                            args,
                            weight: f.weight,
                        }),
                    }
                }
            }
            ItemKind::Query(q) => {
                let pattern = q
                    .atom
                    .args
                    .iter()
                    .map(|t| match t {
                        Term::Const { name } => Some(GroundVal::Sym(dict.intern(name))),
                        Term::Int { value } => Some(GroundVal::Int(*value)),
                        Term::Var { .. } | Term::Agg { .. } => None,
                    })
                    .collect();
                queries.push(QuerySpec {
                    kind: q.kind,
                    pred: q.atom.pred.clone(),
                    pattern,
                });
            }
            _ => {}
        }
    }
    if diags.has_errors() {
        return Err(diags);
    }

    // 6. Assemble Core-IR predicates (executable ones).
    let predicates = order
        .iter()
        .filter_map(|name| {
            let info = declared[name];
            info.semiring().map(|sem| CorePred {
                name: name.clone(),
                arity: info.arity,
                semiring: sem,
                stratum: stratum[idx[name.as_str()]],
            })
        })
        .collect();

    Ok(Checked {
        core: CoreProgram {
            predicates,
            rules,
            num_strata,
        },
        dict,
        edb,
        prob_edb,
        queries,
    })
}

fn literal_atom(lit: &Literal) -> &Atom {
    match lit {
        Literal::Pos(a) | Literal::Neg(a) => a,
    }
}

/// Predicates that (transitively) depend on themselves — i.e. sit on a cycle in
/// the dependency graph. Recursion classification for table 2.4 (CHECK-6).
fn recursive_preds(n: usize, edges: &[(usize, usize, bool)]) -> std::collections::HashSet<usize> {
    let mut adj = vec![Vec::new(); n];
    for &(u, v, _) in edges {
        adj[u].push(v);
    }
    let mut rec = std::collections::HashSet::new();
    for start in 0..n {
        let mut seen = vec![false; n];
        let mut stack = adj[start].clone();
        while let Some(x) = stack.pop() {
            if x == start {
                rec.insert(start);
                break;
            }
            if !seen[x] {
                seen[x] = true;
                stack.extend(&adj[x]);
            }
        }
    }
    rec
}

/// Iterative stratification. `stratum[u]` grows to satisfy every edge
/// `u → v (strict)`: `stratum[u] ≥ stratum[v] (+1 if strict)`. Converges within
/// `n` passes for a stratifiable program; if a strict edge sits on a cycle the
/// values grow without bound and we report the worst offender. Returns
/// `(stratum, num_strata)` or `Err(bad_node)`.
fn stratify(n: usize, edges: &[(usize, usize, bool)]) -> Result<(Vec<u32>, u32), usize> {
    let mut s = vec![0u32; n];
    for _pass in 0..=n {
        let mut changed = false;
        for &(u, v, strict) in edges {
            let want = s[v] + u32::from(strict);
            if s[u] < want {
                s[u] = want;
                changed = true;
            }
        }
        if !changed {
            let num = s.iter().copied().max().unwrap_or(0) + 1;
            return Ok((s, num));
        }
    }
    Err((0..n).max_by_key(|&i| s[i]).unwrap_or(0))
}

/// Every head variable and negated-literal variable must appear in a positive
/// body literal (range-restriction / safety, CHECK-13).
fn check_range_restriction(r: &strata_ir::high::Rule, span: Span, diags: &mut Diagnostics) {
    let mut pos: std::collections::HashSet<&str> = Default::default();
    for lit in &r.body {
        if let Literal::Pos(a) = lit {
            collect_vars(a, &mut |v| {
                pos.insert(v);
            });
        }
    }
    let mut report = |v: &str| {
        diags.error(
            codes::NOT_RANGE_RESTRICTED,
            format!("variable `{v}` is not bound by a positive body literal"),
            span,
        );
    };
    collect_vars(&r.head, &mut |v| {
        if !pos.contains(v) {
            report(v);
        }
    });
    for lit in &r.body {
        if let Literal::Neg(a) = lit {
            collect_vars(a, &mut |v| {
                if !pos.contains(v) {
                    report(v);
                }
            });
        }
    }
}

/// Each positive body predicate's semiring must be coercible into the head's
/// (Bool ⊑ Trop; Trop cannot flow into Bool). [CHECK-4]
fn check_semirings(
    r: &strata_ir::high::Rule,
    span: Span,
    declared: &HashMap<String, PredInfo>,
    diags: &mut Diagnostics,
) {
    let Some(head_sem) = declared.get(&r.head.pred).and_then(|i| i.semiring()) else {
        return;
    };
    for lit in &r.body {
        if let Literal::Pos(a) = lit {
            if let Some(bs) = declared.get(&a.pred).and_then(|i| i.semiring()) {
                if !coercible(bs, head_sem) {
                    diags.error(
                        codes::SEMIRING_CONFLICT,
                        format!(
                            "`{}` ({bs:?}) cannot flow into `{}` ({head_sem:?}); \
                             use an explicit conversion",
                            a.pred, r.head.pred
                        ),
                        span,
                    );
                }
            }
        }
    }
}

fn collect_vars<'a>(atom: &'a Atom, f: &mut impl FnMut(&'a str)) {
    for t in &atom.args {
        match t {
            Term::Var { name } => f(name),
            Term::Agg { var, .. } => f(var),
            _ => {}
        }
    }
}

fn lower_rule(
    r: &strata_ir::high::Rule,
    declared: &HashMap<String, PredInfo>,
    stratum: &[u32],
    idx: &HashMap<&str, usize>,
    dict: &mut SymbolDict,
) -> Option<CoreRule> {
    // Assign a canonical slot to each variable, in first-appearance order.
    let mut slots: HashMap<String, u32> = HashMap::new();
    let assign = |name: &str, slots: &mut HashMap<String, u32>| -> u32 {
        let n = slots.len() as u32;
        *slots.entry(name.to_string()).or_insert(n)
    };
    // Pre-scan head then body so slots are stable and deterministic.
    for t in &r.head.args {
        match t {
            Term::Var { name } => {
                assign(name, &mut slots);
            }
            Term::Agg { var, .. } => {
                assign(var, &mut slots);
            }
            _ => {}
        }
    }
    for lit in &r.body {
        for t in &literal_atom(lit).args {
            if let Term::Var { name } = t {
                assign(name, &mut slots);
            }
        }
    }

    let lower_atom = |a: &Atom, dict: &mut SymbolDict| CoreAtom {
        pred: a.pred.clone(),
        args: a
            .args
            .iter()
            .map(|t| match t {
                Term::Var { name } => CoreTerm::Var { slot: slots[name] },
                Term::Const { name } => CoreTerm::Const {
                    sym: dict.intern(name),
                },
                Term::Int { value } => CoreTerm::Int { value: *value },
                Term::Agg { op, var } => CoreTerm::Agg {
                    op: *op,
                    slot: slots[var],
                },
            })
            .collect(),
    };

    let head = lower_atom(&r.head, dict);
    let body = r
        .body
        .iter()
        .map(|lit| match lit {
            Literal::Pos(a) => CoreLiteral::Pos(lower_atom(a, dict)),
            Literal::Neg(a) => CoreLiteral::Neg(lower_atom(a, dict)),
        })
        .collect();

    let head_sem = declared.get(&r.head.pred).and_then(|i| i.semiring())?;
    let st = stratum[idx[r.head.pred.as_str()]];
    Some(CoreRule {
        head,
        body,
        stratum: st,
        var_count: slots.len() as u32,
        neg_weight_cycle_check: head_sem == Semiring::Trop,
    })
}

/// Resolve a fact's arguments to ground values (constants interned), or `None`
/// with a diagnostic if the fact is not ground. Weight/probability routing is
/// the caller's job.
fn ground_fact_args(
    f: &strata_ir::high::Fact,
    span: Span,
    dict: &mut SymbolDict,
    diags: &mut Diagnostics,
) -> Option<Vec<GroundVal>> {
    let mut args = Vec::with_capacity(f.atom.args.len());
    for t in &f.atom.args {
        match t {
            Term::Const { name } => args.push(GroundVal::Sym(dict.intern(name))),
            Term::Int { value } => args.push(GroundVal::Int(*value)),
            Term::Var { .. } | Term::Agg { .. } => {
                diags.error(
                    codes::NON_GROUND_FACT,
                    format!("fact `{}` contains a non-ground term", f.atom.pred),
                    span,
                );
                return None;
            }
        }
    }
    Some(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unstratifiable_iteration_detects_negation_cycle() {
        // p depends on not p (self strict cycle): edges (0->0 strict).
        assert_eq!(stratify(1, &[(0, 0, true)]), Err(0));
        // positive self-loop is fine (same stratum).
        assert_eq!(stratify(1, &[(0, 0, false)]), Ok((vec![0], 1)));
    }

    #[test]
    fn strata_are_assigned_by_strict_depth() {
        // 0 -> 1 (strict): stratum(0)=1, stratum(1)=0.
        assert_eq!(stratify(2, &[(0, 1, true)]), Ok((vec![1, 0], 2)));
    }
}
