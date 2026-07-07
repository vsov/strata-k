//! The in-process neural boundary, end to end: a model object's forward pass
//! supplies the soft facts — nothing is pasted into the program text — and the
//! query's gradient flows back toward the model by position.
//!
//!   cargo run -p strata-k --example neural_inprocess

use strata_ir::value::GroundVal;
use strata_k::{attach_models, compile, grad_query, Model, SymbolDict, Tuple};

/// A stand-in for a real network: scores firms from an in-memory feature
/// table. Swap the body for a torch/onnx call and nothing else changes.
struct RiskModel;

impl Model for RiskModel {
    fn name(&self) -> &str {
        "risk_gnn"
    }
    fn soft_facts(&self, dict: &mut SymbolDict) -> Vec<(String, Tuple, f64)> {
        [("acme", 0.9), ("globex", 0.2)]
            .into_iter()
            .map(|(firm, p)| {
                (
                    "flag".to_string(),
                    vec![GroundVal::Sym(dict.intern(firm))],
                    p,
                )
            })
            .collect()
    }
}

fn main() {
    let mut program = compile(
        "domain firm.\n\
         neural flag(firm) from model \"risk_gnn\".\n\
         pred investigate(firm): Prov.\n\
         investigate(X) :- flag(X).\n",
    )
    .expect("checks");

    // The forward pass runs here — the facts are computed, not pasted.
    attach_models(&mut program, &[&RiskModel]).expect("model attaches");

    let answers = grad_query(&mut program, "investigate", &[None]).expect("grad");
    for (tuple, p, grad) in answers {
        let firm = match tuple[0] {
            GroundVal::Sym(id) => program.dict.resolve(id).unwrap(),
            _ => unreachable!(),
        };
        println!("{p} :: investigate({firm})");
        for ((pred, args, pw), g) in program.prob_edb.iter().zip(&grad) {
            let a = match args[0] {
                GroundVal::Sym(id) => program.dict.resolve(id).unwrap(),
                _ => unreachable!(),
            };
            println!("  ∂/∂[{pw} :: {pred}({a})] = {g}  (→ model \"risk_gnn\")");
        }
    }
}
