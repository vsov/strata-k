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
use strata_ir::terms::{TermTable, DEFAULT_MAX_DEPTH};
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
    /// Neural predicates and the model each draws its soft facts from
    /// (`neural n(...) from model "m"`). Their facts live in `prob_edb`; `?grad`
    /// differentiates the query back through them into the model.
    pub neural: Vec<(String, String)>,
    /// The hash-cons table for `@terms` compound values: the EDB's compound facts
    /// are interned here, and the evaluator extends it as rule heads build terms.
    pub terms: TermTable,
    /// Each predicate's declared annotation. Core-IR carries only the executable
    /// semiring (Prov/Prov_k evaluate set-wise as Bool); the CLI reads this map
    /// to route Prov/Prov_k predicates through provenance capture + the circuit.
    pub annotations: HashMap<String, Annotation>,
}

#[derive(Debug, Clone, Copy)]
struct PredInfo {
    arity: u32,
    annotation: Annotation,
}

impl PredInfo {
    /// The executable Core-IR semiring. Prov/Prov_k tuples are *set-wise* Bool
    /// (which tuples exist is a Bool question; the provenance DNF each tuple
    /// carries lives in the provenance evaluator, not in Core-IR).
    fn semiring(&self) -> Semiring {
        annotation_semiring(&self.annotation)
    }
}

const NOSPAN: Span = Span { start: 0, end: 0 };

fn annotation_semiring(a: &Annotation) -> Semiring {
    match a {
        Annotation::Bool | Annotation::Prov | Annotation::ProvK { .. } => Semiring::Bool,
        Annotation::Trop => Semiring::Trop,
    }
}

/// A surface name for diagnostics.
fn ann_name(a: &Annotation) -> String {
    match a {
        Annotation::Bool => "Bool".into(),
        Annotation::Trop => "Trop".into(),
        Annotation::Prov => "Prov".into(),
        Annotation::ProvK { k } => format!("Prov_k({k})"),
    }
}

/// Check and lower a High-IR program to Core-IR.
pub fn check_program(program: &Program) -> Result<Checked, Diagnostics> {
    let mut diags = Diagnostics::new();

    // 1. Collect declarations (with their clause spans for diagnostics).
    let mut declared: HashMap<String, PredInfo> = HashMap::new();
    let mut declared_span: HashMap<String, Span> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    // Neural predicates → their model (facts are the model's soft outputs).
    let mut neural: Vec<(String, String)> = Vec::new();
    let mut neural_preds: std::collections::HashSet<String> = Default::default();
    for item in &program.items {
        if let ItemKind::Predicate(p) = &item.node {
            let info = PredInfo {
                arity: p.sig.args.len() as u32,
                annotation: p.sig.annotation,
            };
            match declared.get(&p.name) {
                None => {
                    order.push(p.name.clone());
                    declared.insert(p.name.clone(), info);
                    declared_span.insert(p.name.clone(), item.span);
                }
                // A conflicting redeclaration must not silently win (it would
                // make every downstream check order-dependent); an identical
                // one is harmless.
                Some(prev) if prev.arity != info.arity || prev.annotation != info.annotation => {
                    diags.error(
                        codes::CONFLICTING_DECLARATION,
                        format!(
                            "predicate `{}` is redeclared with a conflicting signature \
                             (arity {} : {}, was arity {} : {})",
                            p.name,
                            info.arity,
                            ann_name(&info.annotation),
                            prev.arity,
                            ann_name(&prev.annotation)
                        ),
                        item.span,
                    );
                }
                Some(_) => {}
            }
            if let Some(spec) = &p.neural {
                if neural_preds.insert(p.name.clone()) {
                    neural.push((p.name.clone(), spec.model.clone()));
                }
            }
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
            // `input` onto a neural predicate is legal — the loader demands a
            // trailing probability column, so the rows arrive soft (the
            // certain-row case fails at load time, not silently).
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
            // Non-recursive Prov (exact circuit) and Prov_k (top-k, recursion
            // included) are режим B — executed by capture + compilation.
            Annotation::Prov => {}
            Annotation::ProvK { k: 0 } => diags.error(
                codes::NOT_EXECUTABLE,
                format!("`Prov_k(0)` on `{name}` keeps no proofs; the bound must be ≥ 1"),
                span,
            ),
            Annotation::ProvK { .. } => {}
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
    let mut terms = TermTable::new(DEFAULT_MAX_DEPTH);

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
                // The fact's `::` annotation must fit the predicate's declared
                // annotation (CHECK, E1009): an integer weight is Trop-only, a
                // probability is soft-only (Bool/Prov/Prov_k) and in [0, 1],
                // and a Trop fact must carry a weight — a bare Trop fact would
                // seed a weightless tuple the tropical fixpoint cannot combine.
                if !check_fact_annotation(f, item.span, &declared, &mut diags) {
                    continue;
                }
                if let Some(args) =
                    ground_fact_args(f, item.span, &mut dict, &mut terms, &mut diags)
                {
                    match f.prob {
                        Some(p) => prob_edb.push((f.atom.pred.clone(), args, p)),
                        None => {
                            // A neural predicate's atoms are the model's soft
                            // outputs — a certain fact on one is a category error.
                            if neural_preds.contains(&f.atom.pred) {
                                diags.error(
                                    codes::NEURAL_FACT_NOT_SOFT,
                                    format!(
                                        "`{}` is a neural predicate; its facts are the model's \
                                         soft outputs and must be probabilistic (`p :: {}(...)`)",
                                        f.atom.pred, f.atom.pred
                                    ),
                                    item.span,
                                );
                            } else {
                                edb.push(GroundFact {
                                    pred: f.atom.pred.clone(),
                                    args,
                                    weight: f.weight,
                                });
                            }
                        }
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
                        // A ground compound query pattern is resolved at run time
                        // against the term table; treat as an open position here.
                        Term::Var { .. } | Term::Agg { .. } | Term::Compound { .. } => None,
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

    // 6. Assemble Core-IR predicates.
    let predicates = order
        .iter()
        .map(|name| {
            let info = declared[name];
            CorePred {
                name: name.clone(),
                arity: info.arity,
                semiring: info.semiring(),
                stratum: stratum[idx[name.as_str()]],
            }
        })
        .collect();
    let annotations = declared
        .iter()
        .map(|(name, info)| (name.clone(), info.annotation))
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
        neural,
        terms,
        annotations,
    })
}

fn literal_atom(lit: &Literal) -> &Atom {
    match lit {
        Literal::Pos(a) | Literal::Neg(a) => a,
    }
}

/// Assign a canonical slot to each variable a term mentions, recursing into
/// compound terms (`@terms`) so a variable inside `cons(X, Xs)` is bound too.
fn assign_vars(t: &Term, slots: &mut HashMap<String, u32>) {
    match t {
        Term::Var { name } => {
            let n = slots.len() as u32;
            slots.entry(name.clone()).or_insert(n);
        }
        Term::Agg { var, .. } => {
            let n = slots.len() as u32;
            slots.entry(var.clone()).or_insert(n);
        }
        Term::Compound { args, .. } => {
            for a in args {
                assign_vars(a, slots);
            }
        }
        Term::Const { .. } | Term::Int { .. } => {}
    }
}

/// Lower a High-IR term to Core-IR, recursively for compounds (`@terms`).
fn lower_term(t: &Term, slots: &HashMap<String, u32>, dict: &mut SymbolDict) -> CoreTerm {
    match t {
        Term::Var { name } => CoreTerm::Var { slot: slots[name] },
        Term::Const { name } => CoreTerm::Const {
            sym: dict.intern(name),
        },
        Term::Int { value } => CoreTerm::Int { value: *value },
        Term::Agg { op, var } => CoreTerm::Agg {
            op: *op,
            slot: slots[var],
        },
        Term::Compound { functor, args } => CoreTerm::Compound {
            functor: dict.intern(functor),
            args: args.iter().map(|a| lower_term(a, slots, dict)).collect(),
        },
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

/// Declaration and arity checks for `@asp` modules. Stable-model semantics
/// legitimately skips stratification (unstratified negation is the point) and
/// the semiring machinery (ASP is Bool), but the mandatory-signature promise —
/// a mistyped predicate is a compile error, never a silently empty relation —
/// is a *global* property of the language, so `@asp` does not get to bypass it.
pub fn check_asp_declarations(program: &Program) -> Result<(), Diagnostics> {
    let mut diags = Diagnostics::new();
    let mut declared: HashMap<String, u32> = HashMap::new();
    for item in &program.items {
        if let ItemKind::Predicate(p) = &item.node {
            let arity = p.sig.args.len() as u32;
            if let Some(&prev) = declared.get(&p.name) {
                if prev != arity {
                    diags.error(
                        codes::CONFLICTING_DECLARATION,
                        format!(
                            "predicate `{}` is redeclared with arity {} (was {})",
                            p.name, arity, prev
                        ),
                        item.span,
                    );
                }
            } else {
                declared.insert(p.name.clone(), arity);
            }
            // A neural predicate's facts are soft; @asp has no soft facts.
            if p.neural.is_some() {
                diags.error(
                    codes::ASP_UNSUPPORTED,
                    format!(
                        "`neural {}` is not supported under `@asp`: stable-model \
                         semantics has no soft facts",
                        p.name
                    ),
                    item.span,
                );
            }
        }
    }
    let mut reported: std::collections::HashSet<String> = Default::default();
    let mut check_atom =
        |atom: &Atom, span: Span, diags: &mut Diagnostics| match declared.get(&atom.pred) {
            None => {
                if reported.insert(atom.pred.clone()) {
                    diags.error(
                        codes::UNDECLARED_PRED,
                        format!("predicate `{}` is used but never declared", atom.pred),
                        span,
                    );
                }
            }
            Some(&arity) => {
                if arity as usize != atom.args.len() {
                    diags.error(
                        codes::ARITY_MISMATCH,
                        format!(
                            "predicate `{}` expects {} argument(s), found {}",
                            atom.pred,
                            arity,
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
            ItemKind::Fact(f) => {
                check_atom(&f.atom, item.span, &mut diags);
                // @asp facts are certain Bool atoms: a `::` annotation would be
                // silently reinterpreted as certainty — refuse it by name.
                if f.weight.is_some() || f.prob.is_some() {
                    diags.error(
                        codes::ASP_UNSUPPORTED,
                        format!(
                            "`:: {}(...)` — fact annotations (weights/probabilities) are \
                             not supported under `@asp`; stable-model facts are certain",
                            f.atom.pred
                        ),
                        item.span,
                    );
                }
                // The grounder instantiates over constants/integers only; a
                // variable or compound argument would be silently dropped.
                for t in &f.atom.args {
                    match t {
                        Term::Const { .. } | Term::Int { .. } => {}
                        Term::Var { .. } | Term::Agg { .. } => {
                            diags.error(
                                codes::NON_GROUND_FACT,
                                format!("fact `{}` contains a non-ground term", f.atom.pred),
                                item.span,
                            );
                            break;
                        }
                        Term::Compound { .. } => {
                            diags.error(
                                codes::ASP_UNSUPPORTED,
                                format!(
                                    "fact `{}` carries a compound term; `@terms` values are \
                                     not supported under `@asp`",
                                    f.atom.pred
                                ),
                                item.span,
                            );
                            break;
                        }
                    }
                }
            }
            ItemKind::Query(q) => {
                // The ASP runner enumerates stable models; it answers no queries.
                diags.error(
                    codes::ASP_UNSUPPORTED,
                    format!(
                        "`? {}(...)` — queries are not supported under `@asp`; the run \
                         enumerates the stable models instead",
                        q.atom.pred
                    ),
                    item.span,
                );
            }
            ItemKind::Input(inp) => {
                // The @asp runner never loads TSV EDBs; a silently empty
                // relation is exactly what the signature promise forbids.
                diags.error(
                    codes::ASP_UNSUPPORTED,
                    format!(
                        "`input {} from ...` is not supported under `@asp`; \
                         inline the facts",
                        inp.pred
                    ),
                    item.span,
                );
            }
            _ => {}
        }
    }
    if diags.has_errors() {
        Err(diags)
    } else {
        Ok(())
    }
}

/// A fact's `::` annotation against the predicate's declared annotation
/// (E1009). Returns whether the fact is well-formed (callers skip lowering a
/// bad fact so it cannot leak a mistyped annotation into the EDB). [CHECK]
fn check_fact_annotation(
    f: &strata_ir::high::program::Fact,
    span: Span,
    declared: &HashMap<String, PredInfo>,
    diags: &mut Diagnostics,
) -> bool {
    let Some(info) = declared.get(&f.atom.pred) else {
        return true; // undeclared already reported (E1001)
    };
    let pred = &f.atom.pred;
    let ann = ann_name(&info.annotation);
    let is_trop = matches!(info.annotation, Annotation::Trop);
    if f.weight.is_some() {
        if !is_trop {
            diags.error(
                codes::FACT_ANNOTATION_MISMATCH,
                format!(
                    "the fact on `{pred}(...)` carries an integer (tropical) weight, but \
                     `{pred}` is declared {ann}; a probability needs a decimal point \
                     (`0.5 ::`), a weight needs the predicate annotated `Trop`"
                ),
                span,
            );
            return false;
        }
    } else if let Some(p) = f.prob {
        if is_trop {
            diags.error(
                codes::FACT_ANNOTATION_MISMATCH,
                format!(
                    "`{p} :: {pred}(...)` is a probability, but `{pred}` is declared `Trop`; \
                     a tropical weight is an integer (`5 ::`)"
                ),
                span,
            );
            return false;
        }
        if !(0.0..=1.0).contains(&p) {
            diags.error(
                codes::FACT_ANNOTATION_MISMATCH,
                format!("probability {p} on `{pred}(...)` is outside [0, 1]"),
                span,
            );
            return false;
        }
    } else if is_trop {
        diags.error(
            codes::FACT_ANNOTATION_MISMATCH,
            format!(
                "`{pred}` is declared `Trop`, so its facts must carry an integer weight \
                 (`5 :: {pred}(...)`)"
            ),
            span,
        );
        return false;
    }
    true
}

/// Each positive body predicate's annotation must be coercible into the head's
/// along the 1.7 lattice: `Bool ⊑ Trop`, `Bool ⊑ Prov ⊑ Prov_k`; `Trop` and
/// `Prov` are incomparable, and soft (`Prov`/`Prov_k`) evidence can never be
/// laundered back into `Bool`/`Trop` — the taint discipline. [CHECK-4/5]
fn check_semirings(
    r: &strata_ir::high::Rule,
    span: Span,
    declared: &HashMap<String, PredInfo>,
    diags: &mut Diagnostics,
) {
    let Some(head_ann) = declared.get(&r.head.pred).map(|i| i.annotation) else {
        return;
    };
    for lit in &r.body {
        if let Literal::Pos(a) = lit {
            if let Some(body_ann) = declared.get(&a.pred).map(|i| i.annotation) {
                if !body_ann.is_coercible_to(&head_ann) {
                    diags.error(
                        codes::SEMIRING_CONFLICT,
                        format!(
                            "`{}` ({}) cannot flow into `{}` ({}); \
                             use an explicit conversion",
                            a.pred,
                            ann_name(&body_ann),
                            r.head.pred,
                            ann_name(&head_ann)
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
        collect_term_vars(t, f);
    }
}

/// Collect variables of a term, recursing into compound terms (`@terms`) so a
/// variable inside `box(Y)` counts for range-restriction and safety.
fn collect_term_vars<'a>(t: &'a Term, f: &mut impl FnMut(&'a str)) {
    match t {
        Term::Var { name } => f(name),
        Term::Agg { var, .. } => f(var),
        Term::Compound { args, .. } => {
            for a in args {
                collect_term_vars(a, f);
            }
        }
        Term::Const { .. } | Term::Int { .. } => {}
    }
}

fn lower_rule(
    r: &strata_ir::high::Rule,
    declared: &HashMap<String, PredInfo>,
    stratum: &[u32],
    idx: &HashMap<&str, usize>,
    dict: &mut SymbolDict,
) -> Option<CoreRule> {
    // Assign a canonical slot to each variable, in first-appearance order,
    // recursing into compound terms (`@terms`) so `cons(X, Xs)` binds X and Xs.
    let mut slots: HashMap<String, u32> = HashMap::new();
    for t in &r.head.args {
        assign_vars(t, &mut slots);
    }
    for lit in &r.body {
        for t in &literal_atom(lit).args {
            assign_vars(t, &mut slots);
        }
    }

    let lower_atom = |a: &Atom, dict: &mut SymbolDict| CoreAtom {
        pred: a.pred.clone(),
        args: a.args.iter().map(|t| lower_term(t, &slots, dict)).collect(),
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

    let head_sem = declared.get(&r.head.pred).map(|i| i.semiring())?;
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
    terms: &mut TermTable,
    diags: &mut Diagnostics,
) -> Option<Vec<GroundVal>> {
    let mut args = Vec::with_capacity(f.atom.args.len());
    for t in &f.atom.args {
        match ground_fact_term(t, dict, terms) {
            Ok(v) => args.push(v),
            Err(()) => {
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

/// Resolve one fact term to a ground value, interning compound (`@terms`) terms
/// into the table. `Err` on a variable/aggregate (a fact must be ground).
fn ground_fact_term(
    t: &Term,
    dict: &mut SymbolDict,
    terms: &mut TermTable,
) -> Result<GroundVal, ()> {
    match t {
        Term::Const { name } => Ok(GroundVal::Sym(dict.intern(name))),
        Term::Int { value } => Ok(GroundVal::Int(*value)),
        Term::Compound { functor, args } => {
            let functor = dict.intern(functor);
            let mut gargs = Vec::with_capacity(args.len());
            for a in args {
                gargs.push(ground_fact_term(a, dict, terms)?);
            }
            // Facts are finite, but honour the bound anyway (sound-but-incomplete).
            terms
                .intern(functor, gargs)
                .map(GroundVal::Term)
                .map_err(|_| ())
        }
        Term::Var { .. } | Term::Agg { .. } => Err(()),
    }
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
