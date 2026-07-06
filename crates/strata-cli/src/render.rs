//! Canonical text rendering: relations, values, weights, and the
//! `p :: fact` marginal line shared by the режим-B paths.

use strata_eval::{Ann, Db, GroundVal};
use strata_ir::dict::SymbolDict;
use strata_ir::terms::TermTable;
use strata_ir::trop::Weight;

pub(crate) fn prob_line(p: f64, pred: &str, tuple: &[GroundVal], dict: &SymbolDict) -> String {
    let args: Vec<String> = tuple.iter().map(|v| render_val(v, dict)).collect();
    format!("{p} :: {pred}({})\n", args.join(", "))
}

/// Canonical output: relations in name order, tuples sorted (BTreeMap), constants
/// resolved through the dictionary (IR-10). [render half of IR-10]
pub(crate) fn render_db(db: &Db, dict: &SymbolDict, terms: &TermTable) -> String {
    let mut out = String::new();
    for pred in db.predicates() {
        let rel = db.relation(pred).unwrap();
        for (tuple, ann) in &rel.rows {
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

/// Term-free render (probabilistic / gradient paths never produce `@terms`).
pub(crate) fn render_val(v: &GroundVal, dict: &SymbolDict) -> String {
    match v {
        GroundVal::Sym(id) => dict.resolve(*id).unwrap_or("?").to_string(),
        GroundVal::Int(n) => n.to_string(),
        GroundVal::Term(id) => format!("<term#{}>", id.0),
    }
}

pub(crate) fn render_weight(w: Weight) -> String {
    match w {
        Weight::Finite(n) => n.to_string(),
        Weight::PosInf => strata_ir::output::POS_INF_TOKEN.to_string(),
    }
}
