//! `strata-k` — the library facade over the Strata/K reference stack.
//!
//! The CLI is one consumer of the engine; this crate is the front door for the
//! others: embed the pipeline (`parse → check → run`), ask probabilistic and
//! gradient queries, capture provenance, enumerate stable models — and wire a
//! **model in-process**, so a neural predicate's soft facts are *computed* at
//! evaluation time instead of pasted into the program text.
//!
//! ```
//! use strata_k::{compile, eval};
//!
//! let mut program = compile(
//!     "pred edge(node, node): Bool.\n\
//!      pred path(node, node): Bool.\n\
//!      path(X, Y) :- edge(X, Y).\n\
//!      path(X, Z) :- edge(X, Y), path(Y, Z).\n\
//!      edge(a, b).\n\
//!      edge(b, c).\n",
//! )
//! .expect("checks");
//! let db = eval(&mut program).expect("runs");
//! assert_eq!(db.relation("path").unwrap().len(), 3);
//! ```
//!
//! Everything here is a thin, typed veneer: the semantics live in the
//! reference crates (`strata-front`, `strata-check`, `strata-eval`,
//! `strata-asp`, `strata-prob`), which stay independently usable.

use std::collections::HashMap;

pub mod input;

use strata_check::Checked;
use strata_eval::provenance::{ProvDb, ProvMode};
use strata_eval::{run_prov, run_terms, Ann, Db, GroundVal};
use strata_ir::high::sig::Annotation;
use strata_ir::value::GroundFact;

pub use input::load_inputs;
pub use strata_check::{check_asp_declarations, check_program, Diagnostics};
pub use strata_eval::{EvalError, ProbError, ProvError};
pub use strata_front::{format, parse, print_program};
pub use strata_ir::dict::SymbolDict;
pub use strata_prob::{compile_exact, BudgetExceeded, Circuit};

/// One row of a relation. Symbols are already interned; resolve them through
/// the program's [`SymbolDict`] (`checked.dict`).
pub type Tuple = Vec<GroundVal>;

/// Parse and check a program in one step: the `text → High-IR → Core-IR` half
/// of the pipeline. All diagnostics (E0xxx and E1xxx) come back typed.
///
/// `input pred from "file"` declarations are *not* loaded here (checking is
/// pure); call [`load_inputs`] with the base directory to resolve them.
pub fn compile(src: &str) -> Result<Checked, Diagnostics> {
    let (prog, diags) = parse(src);
    if diags.has_errors() {
        return Err(diags);
    }
    check_program(&prog)
}

/// Run a checked program to its least fixpoint (naive `T_P`, `@terms`-aware).
/// `checked` is borrowed mutably because constructed compound terms extend its
/// hash-cons table (they must outlive the database to be rendered).
///
/// **Certain slice only**: probabilistic facts (`p :: ...`, attached models)
/// do not participate — they are what [`prob_query`]/[`grad_query`]/
/// [`provenance`] consume. A soft-only program evaluates to empty relations
/// here, by design, not by accident.
pub fn eval(checked: &mut Checked) -> Result<Db, EvalError> {
    let edb: Vec<(&str, Tuple, Ann)> = checked
        .edb
        .iter()
        .map(|f| (f.pred.as_str(), f.args.clone(), Ann::from_weight(f.weight)))
        .collect();
    let core = checked.core.clone();
    run_terms(&core, &edb, &mut checked.terms)
}

/// The exact marginal of every tuple of `pred` matching `pattern` (`None` = any
/// position), by possible-world enumeration — the режим-B oracle. Bool-only,
/// refused past 20 probabilistic facts (`2^n` worlds).
pub fn prob_query(
    checked: &Checked,
    pred: &str,
    pattern: &[Option<GroundVal>],
) -> Result<Vec<(Tuple, f64)>, ProbError> {
    let certain = certain_of(checked);
    strata_eval::prob::query(&checked.core, &certain, &checked.prob_edb, pred, pattern)
}

/// [`prob_query`] plus the gradient of each marginal with respect to every
/// probabilistic fact (`grad[i] = ∂P/∂p_i`, aligned with `checked.prob_edb`).
pub fn grad_query(
    checked: &Checked,
    pred: &str,
    pattern: &[Option<GroundVal>],
) -> Result<Vec<(Tuple, f64, Vec<f64>)>, ProbError> {
    let certain = certain_of(checked);
    strata_eval::prob::grad_query(&checked.core, &certain, &checked.prob_edb, pred, pattern)
}

/// Provenance capture (`Prov`/`Prov_k` stage 1): every derived tuple's minimal
/// proof DNF over the probabilistic facts, with `Prov_k` predicates pruned to
/// their declared top-k. Compile a tuple's proofs with
/// [`compile_exact`] and count with [`Circuit::wmc`]/[`Circuit::grad`].
pub fn provenance(checked: &Checked) -> Result<ProvDb, ProvError> {
    let certain = certain_of(checked);
    run_prov(
        &checked.core,
        &certain,
        &checked.prob_edb,
        &prov_modes(checked),
    )
}

/// One ground atom of a stable model: predicate name and argument values.
pub type AspAtom = (String, Vec<strata_asp::Val>);
/// One stable model: its atoms, sorted.
pub type AspModel = Vec<AspAtom>;

/// The stable models of an `@asp` program source, each model a sorted list of
/// ground atoms. Runs the same declaration checks as the CLI first.
pub fn asp_models(src: &str) -> Result<Vec<AspModel>, AspFacadeError> {
    use strata_ir::high::program::{ItemKind, Term};
    let (prog, diags) = parse(src);
    if diags.has_errors() {
        return Err(AspFacadeError::Diagnostics(diags));
    }
    if let Err(diags) = check_asp_declarations(&prog) {
        return Err(AspFacadeError::Diagnostics(diags));
    }
    let mut rules = Vec::new();
    let mut facts: Vec<(String, Vec<strata_asp::Val>)> = Vec::new();
    for item in &prog.items {
        match &item.node {
            ItemKind::Rule(r) => rules.push(r.clone()),
            ItemKind::Fact(f) => {
                let args = f
                    .atom
                    .args
                    .iter()
                    .map(|t| match t {
                        Term::Const { name } => strata_asp::Val::Sym(name.clone()),
                        Term::Int { value } => strata_asp::Val::Int(*value),
                        // check_asp_declarations already refused anything else.
                        _ => unreachable!("non-ground @asp fact survived checking"),
                    })
                    .collect();
                facts.push((f.atom.pred.clone(), args));
            }
            _ => {}
        }
    }
    strata_asp::solve(&rules, &facts, &[]).map_err(AspFacadeError::Asp)
}

/// An error from [`asp_models`]: either the program didn't check, or the
/// solver refused (grounding bounds, unsupported construct).
#[derive(Debug)]
pub enum AspFacadeError {
    Diagnostics(Diagnostics),
    Asp(strata_asp::AspError),
}

impl std::fmt::Display for AspFacadeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AspFacadeError::Diagnostics(_) => write!(f, "the program did not check"),
            AspFacadeError::Asp(e) => write!(f, "{e:?}"),
        }
    }
}

impl std::error::Error for AspFacadeError {}

// --- the in-process neural boundary ------------------------------------------

/// A model whose forward pass supplies a neural predicate's soft facts — the
/// in-process wiring of the `neural n(...) from model "m"` declaration. The
/// engine calls [`Model::soft_facts`] once per run; each returned atom becomes
/// a probabilistic fact of the predicate(s) declared against this model, and
/// [`grad_query`] gradients flow back to it by position.
pub trait Model {
    /// The name the program binds with `from model "<name>"`.
    fn name(&self) -> &str;
    /// The forward pass: ground atoms with confidences, interned through the
    /// program's dictionary. Every atom must belong to a predicate declared
    /// `neural ... from model "<name>"`; probabilities must lie in [0, 1].
    fn soft_facts(&self, dict: &mut SymbolDict) -> Vec<(String, Tuple, f64)>;
}

/// Wiring errors from [`attach_models`].
#[derive(Debug, Clone, PartialEq)]
pub enum ModelError {
    /// The program declares `from model "name"` but no such model was passed.
    MissingModel(String),
    /// A model produced facts for a predicate not declared against it.
    WrongPredicate { model: String, pred: String },
    /// A model produced a tuple whose arity does not match the declaration —
    /// accepted silently it would never unify, and every query would quietly
    /// return nothing.
    WrongArity {
        model: String,
        pred: String,
        expected: u32,
        got: usize,
    },
    /// A model produced a probability outside [0, 1].
    BadProbability { model: String, p: f64 },
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelError::MissingModel(m) => {
                write!(f, "the program needs model {m:?} but none was attached")
            }
            ModelError::WrongPredicate { model, pred } => write!(
                f,
                "model {model:?} produced facts for `{pred}`, which is not declared \
                 `neural ... from model {model:?}`"
            ),
            ModelError::WrongArity {
                model,
                pred,
                expected,
                got,
            } => write!(
                f,
                "model {model:?} produced a {got}-ary tuple for `{pred}`, declared \
                 with arity {expected}"
            ),
            ModelError::BadProbability { model, p } => {
                write!(f, "model {model:?} produced probability {p} outside [0, 1]")
            }
        }
    }
}

impl std::error::Error for ModelError {}

/// Run every model's forward pass and append the outputs to the program's
/// probabilistic EDB — the facts are *computed*, not pasted. Call before
/// [`prob_query`]/[`grad_query`]/[`provenance`]. Every `from model "m"`
/// declaration must be covered; extra models are ignored.
pub fn attach_models(checked: &mut Checked, models: &[&dyn Model]) -> Result<(), ModelError> {
    let by_name: HashMap<&str, &&dyn Model> = models.iter().map(|m| (m.name(), m)).collect();
    // The predicates each model may speak for, with models kept in the
    // program's declaration order — so `prob_edb` (and therefore gradient
    // indices) is deterministic run to run.
    let mut order: Vec<&str> = Vec::new();
    let mut preds_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for (pred, model) in &checked.neural {
        let e = preds_of.entry(model.as_str()).or_default();
        if e.is_empty() {
            order.push(model.as_str());
        }
        e.push(pred);
    }
    for model_name in &order {
        if !by_name.contains_key(*model_name) {
            return Err(ModelError::MissingModel(model_name.to_string()));
        }
    }
    // Two loops so the dictionary borrow doesn't overlap the neural map borrow.
    let arity_of: HashMap<&str, u32> = checked
        .core
        .predicates
        .iter()
        .map(|p| (p.name.as_str(), p.arity))
        .collect();
    let mut new_facts: Vec<(String, Tuple, f64)> = Vec::new();
    for model_name in &order {
        let model = by_name[model_name];
        for (pred, tuple, p) in model.soft_facts(&mut checked.dict) {
            if !preds_of[model_name].contains(&pred.as_str()) {
                return Err(ModelError::WrongPredicate {
                    model: model_name.to_string(),
                    pred,
                });
            }
            let expected = arity_of.get(pred.as_str()).copied().unwrap_or(0);
            if tuple.len() != expected as usize {
                return Err(ModelError::WrongArity {
                    model: model_name.to_string(),
                    pred,
                    expected,
                    got: tuple.len(),
                });
            }
            if !(0.0..=1.0).contains(&p) {
                return Err(ModelError::BadProbability {
                    model: model_name.to_string(),
                    p,
                });
            }
            new_facts.push((pred, tuple, p));
        }
    }
    checked.prob_edb.extend(new_facts);
    Ok(())
}

// --- helpers ------------------------------------------------------------------

fn certain_of(checked: &Checked) -> Vec<(String, Tuple)> {
    checked
        .edb
        .iter()
        .map(|f: &GroundFact| (f.pred.clone(), f.args.clone()))
        .collect()
}

/// The capture modes the program's annotations declare (`Prov` exact,
/// `Prov_k(k)` top-k) — what [`provenance`] runs with.
pub fn prov_modes(checked: &Checked) -> HashMap<String, ProvMode> {
    checked
        .annotations
        .iter()
        .filter_map(|(name, a)| match a {
            Annotation::Prov => Some((name.clone(), ProvMode::Exact)),
            Annotation::ProvK { k } => Some((name.clone(), ProvMode::TopK(*k))),
            _ => None,
        })
        .collect()
}
