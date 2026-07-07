//! `strata-py` — the Python bridge over the [`strata_k`] facade.
//!
//! ```python
//! import strata_k
//!
//! p = strata_k.compile("""
//!     domain node.
//!     pred edge(node, node): Bool.
//!     pred path(node, node): Bool.
//!     path(X, Y) :- edge(X, Y).
//!     path(X, Z) :- edge(X, Y), path(Y, Z).
//!     edge(a, b). edge(b, c).
//! """)
//! db = p.eval()
//! assert ("a", "c") in db["path"]
//! ```
//!
//! Value mapping: symbols ⇄ `str`, integers ⇄ `int`, compound `@terms` values
//! render structurally as `str` (`"f(a, 3)"`, output only). Trop rows carry a
//! trailing weight (`int`, or `math.inf` for the tropical ⊤). Everything else
//! is a thin, typed veneer over `strata-k` — the semantics live there.

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};
use pyo3::IntoPyObjectExt;

use strata_check::Checked;
use strata_eval::{Ann, GroundVal};
use strata_ir::dict::SymbolDict;
use strata_ir::high;
use strata_ir::terms::TermTable;
use strata_ir::trop::Weight;
use strata_k::{Model, Tuple};

/// A compiled (parsed + checked) Strata/K program: the Python face of
/// [`strata_check::Checked`], plus the parsed source that `load_inputs`
/// resolves `input ... from "file"` declarations against.
#[pyclass]
struct Program {
    high: high::Program,
    checked: Checked,
}

/// A Python argument value headed into the engine: `int` or `str` (symbol).
/// Compound `@terms` values cannot be constructed from Python — they only come
/// *out* (rendered); the term table is program-owned.
#[derive(FromPyObject)]
enum PyIn {
    #[pyo3(transparent)]
    Int(i64),
    #[pyo3(transparent)]
    Sym(String),
}

impl PyIn {
    fn to_val(&self, dict: &mut SymbolDict) -> GroundVal {
        match self {
            PyIn::Int(n) => GroundVal::Int(*n),
            PyIn::Sym(s) => GroundVal::Sym(dict.intern(s)),
        }
    }
}

fn runtime_err<E: std::fmt::Display>(e: E) -> PyErr {
    PyRuntimeError::new_err(e.to_string())
}

/// Render one engine value into Python: `Sym → str`, `Int → int`,
/// `Term → str` (structural, via the program's term table).
fn val_to_py(
    py: Python<'_>,
    v: &GroundVal,
    dict: &SymbolDict,
    terms: &TermTable,
) -> PyResult<Py<PyAny>> {
    match v {
        GroundVal::Int(n) => n.into_py_any(py),
        GroundVal::Sym(id) => dict.resolve(*id).unwrap_or("?").into_py_any(py),
        GroundVal::Term(_) => render_val(v, dict, terms).into_py_any(py),
    }
}

/// Structural text of a value — the same shape the CLI prints.
fn render_val(v: &GroundVal, dict: &SymbolDict, terms: &TermTable) -> String {
    match v {
        GroundVal::Sym(id) => dict.resolve(*id).unwrap_or("?").to_string(),
        GroundVal::Int(n) => n.to_string(),
        GroundVal::Term(id) => {
            let (functor, args) = terms.get(*id);
            let inner: Vec<String> = args.iter().map(|a| render_val(a, dict, terms)).collect();
            format!(
                "{}({})",
                dict.resolve(functor).unwrap_or("?"),
                inner.join(", ")
            )
        }
    }
}

/// A whole row as a Python tuple; `weight` (Trop) appends one trailing element.
fn row_to_py(
    py: Python<'_>,
    tuple: &[GroundVal],
    dict: &SymbolDict,
    terms: &TermTable,
    weight: Option<Weight>,
) -> PyResult<Py<PyTuple>> {
    let mut items: Vec<Py<PyAny>> = tuple
        .iter()
        .map(|v| val_to_py(py, v, dict, terms))
        .collect::<PyResult<_>>()?;
    if let Some(w) = weight {
        items.push(match w {
            Weight::Finite(n) => n.into_py_any(py)?,
            Weight::PosInf => f64::INFINITY.into_py_any(py)?,
        });
    }
    Ok(PyTuple::new(py, items)?.unbind())
}

/// Resolve the query pattern: `None` → all-wildcards of the predicate's arity;
/// a list mixes concrete `int`/`str` values with `None` wildcards.
fn resolve_pattern(
    checked: &mut Checked,
    pred: &str,
    pattern: Option<Vec<Option<PyIn>>>,
) -> PyResult<Vec<Option<GroundVal>>> {
    let arity = checked
        .core
        .predicates
        .iter()
        .find(|p| p.name == pred)
        .map(|p| p.arity as usize)
        .ok_or_else(|| PyValueError::new_err(format!("unknown predicate `{pred}`")))?;
    match pattern {
        None => Ok(vec![None; arity]),
        Some(pat) => {
            if pat.len() != arity {
                return Err(PyValueError::new_err(format!(
                    "pattern for `{pred}` has {} position(s), the predicate has arity {arity}",
                    pat.len()
                )));
            }
            Ok(pat
                .iter()
                .map(|slot| slot.as_ref().map(|v| v.to_val(&mut checked.dict)))
                .collect())
        }
    }
}

/// A Python callable's forward pass, pre-run and pre-extracted — so every
/// Python-side failure surfaces as a `PyErr` *before* the engine is involved
/// ([`Model::soft_facts`] has no error channel by design).
struct PyModel {
    name: String,
    facts: Vec<(String, Vec<PyIn>, f64)>,
}

impl Model for PyModel {
    fn name(&self) -> &str {
        &self.name
    }
    fn soft_facts(&self, dict: &mut SymbolDict) -> Vec<(String, Tuple, f64)> {
        self.facts
            .iter()
            .map(|(pred, args, p)| {
                (
                    pred.clone(),
                    args.iter().map(|a| a.to_val(dict)).collect(),
                    *p,
                )
            })
            .collect()
    }
}

#[pymethods]
impl Program {
    /// Least fixpoint of the certain slice: `{pred: [row, ...]}`. Bool rows
    /// are argument tuples; Trop rows append the weight. Soft facts do not
    /// participate — they are what `prob_query`/`grad_query`/`provenance`
    /// consume.
    fn eval(&mut self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        let db = strata_k::eval(&mut self.checked).map_err(runtime_err)?;
        let out = PyDict::new(py);
        for pred in db.predicates() {
            let rel = db.relation(pred).unwrap();
            let rows = PyList::empty(py);
            for (tuple, ann) in &rel.rows {
                let weight = match ann {
                    Ann::Unit => None,
                    Ann::W(w) => Some(*w),
                };
                rows.append(row_to_py(
                    py,
                    tuple,
                    &self.checked.dict,
                    &self.checked.terms,
                    weight,
                )?)?;
            }
            out.set_item(pred, rows)?;
        }
        Ok(out.unbind())
    }

    /// Exact marginals `[(row, p), ...]` of `pred` rows matching `pattern`
    /// (`None` = wildcard; omitted = all rows), by possible-world enumeration
    /// — the режим-B oracle, refused past 20 probabilistic facts.
    #[pyo3(signature = (pred, pattern=None))]
    fn prob_query(
        &mut self,
        py: Python<'_>,
        pred: &str,
        pattern: Option<Vec<Option<PyIn>>>,
    ) -> PyResult<Py<PyList>> {
        let pat = resolve_pattern(&mut self.checked, pred, pattern)?;
        let rows = strata_k::prob_query(&mut self.checked, pred, &pat).map_err(runtime_err)?;
        let out = PyList::empty(py);
        for (tuple, p) in rows {
            let row = row_to_py(py, &tuple, &self.checked.dict, &self.checked.terms, None)?;
            out.append((row, p))?;
        }
        Ok(out.unbind())
    }

    /// `prob_query` plus the gradient: `[(row, p, grads), ...]`, where
    /// `grads[i] = ∂P/∂p_i` for the i-th entry of `prob_facts()`.
    #[pyo3(signature = (pred, pattern=None))]
    fn grad_query(
        &mut self,
        py: Python<'_>,
        pred: &str,
        pattern: Option<Vec<Option<PyIn>>>,
    ) -> PyResult<Py<PyList>> {
        let pat = resolve_pattern(&mut self.checked, pred, pattern)?;
        let rows = strata_k::grad_query(&mut self.checked, pred, &pat).map_err(runtime_err)?;
        let out = PyList::empty(py);
        for (tuple, p, grads) in rows {
            let row = row_to_py(py, &tuple, &self.checked.dict, &self.checked.terms, None)?;
            out.append((row, p, grads))?;
        }
        Ok(out.unbind())
    }

    /// Provenance capture: `{pred: [(row, proofs), ...]}` where each proof is
    /// a list of signed literals `±(i+1)` over `prob_facts()` (negative =
    /// the i-th fact must be *absent*). `Prov_k(k)` predicates arrive pruned
    /// to their declared top-k; compile a row's proofs with `wmc`/`wmc_grad`.
    fn provenance(&mut self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        let prov = strata_k::provenance(&mut self.checked).map_err(runtime_err)?;
        let out = PyDict::new(py);
        for (pred, rows) in &prov.rels {
            let items = PyList::empty(py);
            for (tuple, proofs) in rows {
                let row = row_to_py(py, tuple, &self.checked.dict, &self.checked.terms, None)?;
                let ps: Vec<Vec<i64>> =
                    proofs.iter().map(|p| p.iter().copied().collect()).collect();
                items.append((row, ps))?;
            }
            out.set_item(pred, items)?;
        }
        Ok(out.unbind())
    }

    /// The probabilistic EDB in engine order: `[(pred, row, p), ...]`. Index
    /// `i` here is gradient position `i` and proof literal `±(i+1)`.
    fn prob_facts(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let out = PyList::empty(py);
        for (pred, tuple, p) in &self.checked.prob_edb {
            let row = row_to_py(py, tuple, &self.checked.dict, &self.checked.terms, None)?;
            out.append((pred.as_str(), row, *p))?;
        }
        Ok(out.unbind())
    }

    /// The queries the source declares, in order: `[(kind, pred, pattern)]`
    /// with kind `"plain" | "prob" | "grad"` and `None` for wildcards.
    fn queries(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let out = PyList::empty(py);
        for q in &self.checked.queries {
            let kind = match q.kind {
                high::program::QueryKind::Plain => "plain",
                high::program::QueryKind::Prob => "prob",
                high::program::QueryKind::Grad => "grad",
            };
            let pat = PyList::empty(py);
            for slot in &q.pattern {
                match slot {
                    None => pat.append(py.None())?,
                    Some(v) => {
                        pat.append(val_to_py(py, v, &self.checked.dict, &self.checked.terms)?)?
                    }
                }
            }
            out.append((kind, q.pred.as_str(), pat))?;
        }
        Ok(out.unbind())
    }

    /// Run each callable's forward pass and append its soft facts to the
    /// probabilistic EDB — the in-process `neural ... from model "name"`
    /// wiring. `models` maps model names to zero-argument callables returning
    /// `[(pred, args, p), ...]`. Call once per program.
    fn attach_models(&mut self, models: &Bound<'_, PyDict>) -> PyResult<()> {
        let mut adapters: Vec<PyModel> = Vec::new();
        for (k, v) in models.iter() {
            let name: String = k.extract()?;
            let facts: Vec<(String, Vec<PyIn>, f64)> = v.call0()?.extract()?;
            adapters.push(PyModel { name, facts });
        }
        let refs: Vec<&dyn Model> = adapters.iter().map(|a| a as &dyn Model).collect();
        strata_k::attach_models(&mut self.checked, &refs).map_err(runtime_err)
    }

    /// Resolve every `input pred from "file"` declaration relative to `base`
    /// (TSV/CSV/JSON by extension, columns typed by the declaration).
    fn load_inputs(&mut self, base: &str) -> PyResult<()> {
        strata_k::load_inputs(&self.high, &mut self.checked, std::path::Path::new(base))
            .map_err(PyValueError::new_err)
    }

    fn __repr__(&self) -> String {
        format!(
            "<strata_k.Program: {} predicate(s), {} certain fact(s), {} soft fact(s)>",
            self.checked.core.predicates.len(),
            self.checked.edb.len(),
            self.checked.prob_edb.len()
        )
    }
}

/// Parse and check a program; every diagnostic (E0xxx/E1xxx) raises
/// `ValueError` with the CLI's rendered text.
#[pyfunction]
fn compile(src: &str) -> PyResult<Program> {
    let (prog, diags) = strata_k::parse(src);
    if diags.has_errors() {
        return Err(PyValueError::new_err(diags.render_text(src)));
    }
    match strata_k::check_program(&prog) {
        Ok(checked) => Ok(Program {
            high: prog,
            checked,
        }),
        Err(diags) => Err(PyValueError::new_err(diags.render_text(src))),
    }
}

/// The stable models of an `@asp` program: `[[(pred, args), ...], ...]`,
/// each model sorted. Same declaration checks as the CLI.
#[pyfunction]
fn asp_models(py: Python<'_>, src: &str) -> PyResult<Py<PyList>> {
    let models = strata_k::asp_models(src).map_err(|e| match e {
        strata_k::AspFacadeError::Diagnostics(d) => PyValueError::new_err(d.render_text(src)),
        other => runtime_err(other),
    })?;
    let out = PyList::empty(py);
    for model in models {
        let atoms = PyList::empty(py);
        for (pred, args) in model {
            let row: Vec<Py<PyAny>> = args
                .iter()
                .map(|v| match v {
                    strata_asp::Val::Sym(s) => s.into_py_any(py),
                    strata_asp::Val::Int(n) => n.into_py_any(py),
                })
                .collect::<PyResult<_>>()?;
            atoms.append((pred, PyTuple::new(py, row)?))?;
        }
        out.append(atoms)?;
    }
    Ok(out.unbind())
}

fn check_dnf(proofs: &[Vec<i64>], num_leaves: usize) -> PyResult<()> {
    for proof in proofs {
        for &lit in proof {
            if lit == 0 || lit.unsigned_abs() as usize > num_leaves {
                return Err(PyValueError::new_err(format!(
                    "literal {lit} out of range for {num_leaves} leaf probability(ies) \
                     (want ±1..=±{num_leaves})"
                )));
            }
        }
    }
    Ok(())
}

fn check_probs(probs: &[f64]) -> PyResult<()> {
    for &p in probs {
        if !(0.0..=1.0).contains(&p) {
            return Err(PyValueError::new_err(format!(
                "probability {p} outside [0, 1]"
            )));
        }
    }
    Ok(())
}

/// Exact weighted model count of a proof DNF (signed literals `±(i+1)` over
/// `probs`), through the same Shannon-compiled circuit the CLI uses. This is
/// the reference an external compiler (e.g. an SDD package) is diffed against.
#[pyfunction]
fn wmc(proofs: Vec<Vec<i64>>, probs: Vec<f64>) -> PyResult<f64> {
    check_dnf(&proofs, probs.len())?;
    check_probs(&probs)?;
    let circuit = strata_k::compile_exact(&proofs, probs.len()).map_err(runtime_err)?;
    Ok(circuit.wmc(&probs))
}

/// [`wmc`] plus the gradient with respect to every leaf: `(p, [∂p/∂probs_i])`.
#[pyfunction]
fn wmc_grad(proofs: Vec<Vec<i64>>, probs: Vec<f64>) -> PyResult<(f64, Vec<f64>)> {
    check_dnf(&proofs, probs.len())?;
    check_probs(&probs)?;
    let circuit = strata_k::compile_exact(&proofs, probs.len()).map_err(runtime_err)?;
    Ok(circuit.grad(&probs))
}

/// The `strata_k` Python module (the extension's import name is set by
/// maturin's `module-name`; the Rust lib target stays `strata_py`).
#[pymodule(name = "strata_k")]
fn strata_py_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Program>()?;
    m.add_function(wrap_pyfunction!(compile, m)?)?;
    m.add_function(wrap_pyfunction!(asp_models, m)?)?;
    m.add_function(wrap_pyfunction!(wmc, m)?)?;
    m.add_function(wrap_pyfunction!(wmc_grad, m)?)?;
    Ok(())
}
