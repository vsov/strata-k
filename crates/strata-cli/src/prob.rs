//! режим-B query answering: `?prob` marginals and `?grad` gradients, routed
//! by annotation — `Bool` through the exact enumeration oracle, `Prov`/`Prov_k`
//! through capture + circuit compilation.

use strata_check::Checked;
use strata_eval::prob::{grad_query, query};
use strata_eval::{marginals, GroundVal, ProvDb};
use strata_ir::high::program::QueryKind;
use strata_ir::value::GroundFact;
use strata_prob::compile_exact;

use crate::prov::{capture_once, prov_annotation, prov_prob_line};
use crate::render::{prob_line, render_val};

pub(crate) fn prob_mode(checked: &Checked) -> bool {
    !checked.prob_edb.is_empty() || checked.queries.iter().any(|q| q.kind == QueryKind::Prob)
}

/// Answer probabilistic queries (or dump all marginals) via режим B. [Phase 4]
pub(crate) fn run_prob(checked: &Checked, certain_facts: &[GroundFact]) -> Result<String, String> {
    let certain: Vec<(String, Vec<GroundVal>)> = certain_facts
        .iter()
        .map(|f| (f.pred.clone(), f.args.clone()))
        .collect();
    let prob_edb = &checked.prob_edb;
    let dict = &checked.dict;

    let prob_qs: Vec<_> = checked
        .queries
        .iter()
        .filter(|q| q.kind == QueryKind::Prob)
        .collect();
    let mut out = String::new();

    if prob_qs.is_empty() {
        // No explicit query: print every predicate's marginal, sorted.
        let m = marginals(&checked.core, &certain, prob_edb).map_err(|e| e.to_string())?;
        let mut preds: Vec<&String> = m.keys().collect();
        preds.sort();
        for pred in preds {
            let mut tuples: Vec<(&Vec<GroundVal>, &f64)> = m[pred].iter().collect();
            tuples.sort_by(|a, b| a.0.cmp(b.0));
            for (tuple, p) in tuples {
                out.push_str(&prob_line(*p, pred, tuple, dict));
            }
        }
    } else {
        // A query against a Prov/Prov_k predicate goes through capture +
        // circuit (spec §2.1 stages 1–3); a Bool predicate stays on the exact
        // enumeration oracle.
        let mut captured: Option<ProvDb> = None;
        for q in prob_qs {
            match prov_annotation(checked, &q.pred) {
                Some(ann) => {
                    let dbp = capture_once(&mut captured, checked, certain_facts)?;
                    let probs: Vec<f64> = prob_edb.iter().map(|x| x.2).collect();
                    for (tuple, proofs) in dbp.query(&q.pred, &q.pattern) {
                        let c = compile_exact(&proofs, probs.len()).map_err(|e| e.to_string())?;
                        out.push_str(&prov_prob_line(c.wmc(&probs), &q.pred, &tuple, dict, ann));
                    }
                }
                None => {
                    let ans = query(&checked.core, &certain, prob_edb, &q.pred, &q.pattern)
                        .map_err(|e| e.to_string())?;
                    for (tuple, p) in ans {
                        out.push_str(&prob_line(p, &q.pred, &tuple, dict));
                    }
                }
            }
        }
    }
    Ok(out)
}

pub(crate) fn grad_mode(checked: &Checked) -> bool {
    checked.queries.iter().any(|q| q.kind == QueryKind::Grad)
}

/// Answer `?grad` queries: the marginal probability of each matching tuple and
/// its gradient w.r.t. every probabilistic fact's probability (reverse-mode over
/// the режим-B chain, spec §2.3). [gradient wiring]
pub(crate) fn run_grad(checked: &Checked, certain_facts: &[GroundFact]) -> Result<String, String> {
    let certain: Vec<(String, Vec<GroundVal>)> = certain_facts
        .iter()
        .map(|f| (f.pred.clone(), f.args.clone()))
        .collect();
    let prob_edb = &checked.prob_edb;
    let dict = &checked.dict;

    let mut out = String::new();
    let mut captured: Option<ProvDb> = None;
    for q in checked.queries.iter().filter(|q| q.kind == QueryKind::Grad) {
        // Prov/Prov_k predicates differentiate the compiled circuit (reverse
        // mode over the chain, spec §2.3); Bool predicates differentiate the
        // enumeration oracle. Both report ∂/∂p per probabilistic fact.
        let ans: Vec<(Vec<GroundVal>, f64, Vec<f64>)> = match prov_annotation(checked, &q.pred) {
            Some(_) => {
                let dbp = capture_once(&mut captured, checked, certain_facts)?;
                let probs: Vec<f64> = prob_edb.iter().map(|x| x.2).collect();
                let mut ans = Vec::new();
                for (tuple, proofs) in dbp.query(&q.pred, &q.pattern) {
                    let (p, g) = compile_exact(&proofs, probs.len())
                        .map_err(|e| e.to_string())?
                        .grad(&probs);
                    ans.push((tuple, p, g));
                }
                ans
            }
            None => grad_query(&checked.core, &certain, prob_edb, &q.pred, &q.pattern)
                .map_err(|e| e.to_string())?,
        };
        for (tuple, p, grad) in ans {
            match prov_annotation(checked, &q.pred) {
                Some(a) => out.push_str(&prov_prob_line(p, &q.pred, &tuple, dict, a)),
                None => out.push_str(&prob_line(p, &q.pred, &tuple, dict)),
            }
            // one gradient line per probabilistic fact, labelled by that fact;
            // a neural fact also names the model the gradient backpropagates into.
            for ((pred, ptuple, pw), g) in prob_edb.iter().zip(&grad) {
                let args: Vec<String> = ptuple.iter().map(|v| render_val(v, dict)).collect();
                let model = checked
                    .neural
                    .iter()
                    .find(|(n, _)| n == pred)
                    .map(|(_, m)| format!("  (→ model {m:?})"))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "  ∂/∂[{pw} :: {pred}({})] = {g}{model}\n",
                    args.join(", ")
                ));
            }
        }
    }
    Ok(out)
}
