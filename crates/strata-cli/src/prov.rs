//! Prov / Prov_k — провенанс: capture → compile → WMC, plus the pedigree
//! display (`⇐` lines) a plain `strata run` prints for provenance predicates.

use std::collections::HashMap;

use strata_check::Checked;
use strata_eval::{run_prov, GroundVal, ProvDb, ProvMode};
use strata_ir::dict::SymbolDict;
use strata_ir::high::sig::Annotation;
use strata_ir::value::GroundFact;
use strata_prob::compile_exact;

use crate::render::{prob_line, render_val};

// --- Prov / Prov_k (провенанс: capture → compile → WMC) ----------------------

/// The declared provenance annotation of `pred`, if it has one.
pub(crate) fn prov_annotation<'a>(checked: &'a Checked, pred: &str) -> Option<&'a Annotation> {
    checked
        .annotations
        .get(pred)
        .filter(|a| matches!(a, Annotation::Prov | Annotation::ProvK { .. }))
}

/// Capture modes for every Prov/Prov_k predicate; empty ⇒ no provenance here.
pub(crate) fn prov_modes(checked: &Checked) -> HashMap<String, ProvMode> {
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

/// Run provenance capture at most once per invocation (queries share it).
pub(crate) fn capture_once<'a>(
    slot: &'a mut Option<ProvDb>,
    checked: &Checked,
    certain_facts: &[GroundFact],
) -> Result<&'a ProvDb, String> {
    if slot.is_none() {
        let certain: Vec<(String, Vec<GroundVal>)> = certain_facts
            .iter()
            .map(|f| (f.pred.clone(), f.args.clone()))
            .collect();
        let dbp = run_prov(
            &checked.core,
            &certain,
            &checked.prob_edb,
            &prov_modes(checked),
        )
        .map_err(|e| e.to_string())?;
        *slot = Some(dbp);
    }
    Ok(slot.as_ref().unwrap())
}

/// A marginal line for a provenance predicate: `Prov` is exact, `Prov_k` is a
/// declared lower bound (И4 — the approximation is visible in the output).
pub(crate) fn prov_prob_line(
    p: f64,
    pred: &str,
    tuple: &[GroundVal],
    dict: &SymbolDict,
    ann: &Annotation,
) -> String {
    let args: Vec<String> = tuple.iter().map(|v| render_val(v, dict)).collect();
    match ann {
        Annotation::ProvK { k } => {
            format!(
                "{p} :: {pred}({})  (lower bound, top-{k})\n",
                args.join(", ")
            )
        }
        _ => format!("{p} :: {pred}({})\n", args.join(", ")),
    }
}

/// One pedigree line per proof: the conjunction of the probabilistic facts a
/// derivation rests on (`⊤` = rests on certain facts only; `¬[...]` = the
/// stratified absence of a soft fact — a dual literal).
fn render_proof(proof: &[i64], checked: &Checked, dict: &SymbolDict) -> String {
    if proof.is_empty() {
        return "⊤".to_string();
    }
    let mut lits: Vec<i64> = proof.to_vec();
    lits.sort_by_key(|l| (l.abs(), *l < 0));
    let parts: Vec<String> = lits
        .iter()
        .map(|&l| {
            let (pred, tuple, pw) = &checked.prob_edb[(l.abs() - 1) as usize];
            let args: Vec<String> = tuple.iter().map(|v| render_val(v, dict)).collect();
            let fact = format!("[{pw} :: {pred}({})]", args.join(", "));
            if l < 0 {
                format!("¬{fact}")
            } else {
                fact
            }
        })
        .collect();
    parts.join(" ∧ ")
}

/// Plain `strata run` on a program with Prov/Prov_k predicates: every relation's
/// tuples with their marginals; provenance-annotated predicates additionally
/// show their pedigree, one `⇐` line per captured proof.
pub(crate) fn run_prov_display(
    checked: &Checked,
    certain_facts: &[GroundFact],
) -> Result<String, String> {
    let mut captured = None;
    let dbp = capture_once(&mut captured, checked, certain_facts)?;
    let dict = &checked.dict;
    let probs: Vec<f64> = checked.prob_edb.iter().map(|x| x.2).collect();
    let modes = prov_modes(checked);

    let mut out = String::new();
    for (pred, rel) in &dbp.rels {
        let ann = prov_annotation(checked, pred);
        for tuple in rel.keys() {
            let matches = dbp.query(pred, &vec![None; tuple.len()]);
            let (_, proofs) = matches.iter().find(|(t, _)| t == tuple).unwrap();
            let certain = proofs.iter().any(|p| p.is_empty());
            if certain {
                let args: Vec<String> = tuple.iter().map(|v| render_val(v, dict)).collect();
                out.push_str(&format!("{pred}({})\n", args.join(", ")));
            } else {
                let p = compile_exact(proofs, probs.len())
                    .map_err(|e| e.to_string())?
                    .wmc(&probs);
                match ann {
                    Some(a) => out.push_str(&prov_prob_line(p, pred, tuple, dict, a)),
                    None => out.push_str(&prob_line(p, pred, tuple, dict)),
                }
            }
            if ann.is_some() {
                for proof in proofs {
                    out.push_str(&format!("  ⇐ {}\n", render_proof(proof, checked, dict)));
                }
            }
        }
    }
    let mut provk: Vec<(&String, u32)> = modes
        .iter()
        .filter_map(|(n, m)| match m {
            ProvMode::TopK(k) => Some((n, *k)),
            ProvMode::Exact => None,
        })
        .collect();
    provk.sort();
    for (pred, k) in provk {
        out.push_str(&format!(
            "% status: lower bound (Prov_k) — {pred}: top-{k} proofs per tuple\n"
        ));
    }
    Ok(out)
}
