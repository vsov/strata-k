//! Canonical text rendering: relations, values, weights, and the
//! `p :: fact` marginal line shared by the режим-B paths.

use strata_check::QuerySpec;
use strata_eval::{Ann, Db, GroundVal};
use strata_ir::dict::SymbolDict;
use strata_ir::terms::TermTable;
use strata_ir::trop::Weight;

pub(crate) fn prob_line(
    p: f64,
    pred: &str,
    tuple: &[GroundVal],
    dict: &SymbolDict,
    terms: &TermTable,
) -> String {
    let args: Vec<String> = tuple.iter().map(|v| render_val_t(v, dict, terms)).collect();
    format!("{p} :: {pred}({})\n", args.join(", "))
}

/// Canonical output: relations in name order, tuples sorted (BTreeMap), constants
/// resolved through the dictionary (IR-10). [render half of IR-10]
///
/// `filter` is the program's plain `?q(...)` queries. When empty, the whole
/// database prints (a plain run with no queries). When non-empty, a plain
/// query stops being a no-op and becomes an output filter: only the queried
/// predicates print, and only tuples matching some query's ground positions
/// (a variable / `_` position matches anything).
pub(crate) fn render_db(
    db: &Db,
    dict: &SymbolDict,
    terms: &TermTable,
    filter: &[QuerySpec],
) -> String {
    let mut out = String::new();
    for pred in db.predicates() {
        if !filter.is_empty() && !filter.iter().any(|q| q.pred == *pred) {
            continue;
        }
        let rel = db.relation(pred).unwrap();
        for (tuple, ann) in &rel.rows {
            if !filter.is_empty() && !matches_any(pred, tuple, filter) {
                continue;
            }
            let args: Vec<String> = tuple.iter().map(|v| render_val_t(v, dict, terms)).collect();
            out.push_str(&format!("{pred}({})", args.join(", ")));
            if let Ann::W(w) = ann {
                out.push_str(&format!(" = {}", render_weight(*w)));
            }
            out.push('\n');
        }
    }
    out
}

/// Does a tuple satisfy at least one plain query for its predicate? A `Some`
/// pattern position must equal the tuple value; a `None` (variable / `_`)
/// position matches anything. An arity mismatch never matches.
fn matches_any(pred: &str, tuple: &[GroundVal], filter: &[QuerySpec]) -> bool {
    filter.iter().filter(|q| q.pred == pred).any(|q| {
        q.pattern.len() == tuple.len()
            && q.pattern
                .iter()
                .zip(tuple)
                .all(|(pat, val)| pat.as_ref().is_none_or(|p| p == val))
    })
}

/// Render a ground value; compound terms (`@terms`) are reconstructed structurally
/// from the term table.
pub(crate) fn render_val_t(v: &GroundVal, dict: &SymbolDict, terms: &TermTable) -> String {
    match v {
        GroundVal::Sym(id) => dict.resolve(*id).unwrap_or("?").to_string(),
        GroundVal::Int(n) => n.to_string(),
        GroundVal::Term(id) => {
            let (functor, args) = terms.get(*id);
            let inner: Vec<String> = args.iter().map(|a| render_val_t(a, dict, terms)).collect();
            format!(
                "{}({})",
                dict.resolve(functor).unwrap_or("?"),
                inner.join(", ")
            )
        }
    }
}

pub(crate) fn render_weight(w: Weight) -> String {
    match w {
        Weight::Finite(n) => n.to_string(),
        Weight::PosInf => strata_ir::output::POS_INF_TOKEN.to_string(),
    }
}
