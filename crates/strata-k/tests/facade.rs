//! Facade integration: the library API answers exactly like the CLI paths,
//! and the in-process neural boundary computes soft facts instead of pasting.

use strata_ir::value::GroundVal;
use strata_k::{
    asp_models, attach_models, compile, eval, grad_query, prob_query, provenance, Model,
    ModelError, SymbolDict, Tuple,
};

fn sym(dict: &SymbolDict, name: &str) -> GroundVal {
    GroundVal::Sym(dict.get(name).expect("interned"))
}

#[test]
fn compile_eval_transitive_closure() {
    let mut p = compile(
        "pred edge(node, node): Bool.\n\
         pred path(node, node): Bool.\n\
         path(X, Y) :- edge(X, Y).\n\
         path(X, Z) :- edge(X, Y), path(Y, Z).\n\
         edge(a, b).\nedge(b, c).\nedge(c, d).\n",
    )
    .expect("checks");
    let db = eval(&mut p).expect("runs");
    assert_eq!(db.relation("path").unwrap().len(), 6);
}

#[test]
fn diagnostics_come_back_typed() {
    let diags = compile("pred e(node, node): Bool.\n5 :: e(a, b).\n").unwrap_err();
    assert!(diags
        .items()
        .iter()
        .any(|d| d.code == strata_check::codes::FACT_ANNOTATION_MISMATCH));
}

#[test]
fn prob_and_grad_queries_match_the_known_marginal() {
    let mut p = compile(
        "pred edge(node, node): Bool.\n\
         pred path(node, node): Bool.\n\
         path(X, Y) :- edge(X, Y).\n\
         path(X, Z) :- edge(X, Y), path(Y, Z).\n\
         0.5 :: edge(a, c).\n0.5 :: edge(a, b).\n0.5 :: edge(b, c).\n",
    )
    .expect("checks");
    let pat = [Some(sym(&p.dict, "a")), Some(sym(&p.dict, "c"))];
    let ans = prob_query(&mut p, "path", &pat).expect("prob");
    assert_eq!(ans.len(), 1);
    assert!((ans[0].1 - 0.625).abs() < 1e-12);
    let g = grad_query(&mut p, "path", &pat).expect("grad");
    assert!((g[0].2[0] - 0.75).abs() < 1e-12, "∂/∂ direct edge");
}

#[test]
fn provenance_pedigree_and_circuit_marginal() {
    let mut p = compile(
        "domain firm.\n\
         pred owns(firm, firm): Bool.\n\
         pred controls(firm, firm): Prov.\n\
         0.9 :: owns(acme, shell).\n\
         0.8 :: owns(shell, target).\n\
         0.3 :: owns(acme, target).\n\
         controls(X, Y) :- owns(X, Y).\n\
         controls(X, Z) :- owns(X, Y), owns(Y, Z).\n",
    )
    .expect("checks");
    let dbp = provenance(&mut p).expect("capture");
    let target: Tuple = vec![sym(&p.dict, "acme"), sym(&p.dict, "target")];
    let hits = dbp.query("controls", &[Some(target[0]), Some(target[1])]);
    assert_eq!(hits.len(), 1);
    let (_, proofs) = &hits[0];
    assert_eq!(proofs.len(), 2, "two minimal proofs");
    let probs: Vec<f64> = p.prob_edb.iter().map(|x| x.2).collect();
    let c = strata_k::compile_exact(proofs, probs.len()).expect("compiles");
    assert!((c.wmc(&probs) - 0.804).abs() < 1e-12);
}

#[test]
fn asp_models_enumerates_stable_models() {
    let models = asp_models(
        "@asp.\n\
         pred a(): Bool.\n\
         pred b(): Bool.\n\
         a() :- not b().\n\
         b() :- not a().\n",
    )
    .expect("solves");
    assert_eq!(models.len(), 2, "{{a}} and {{b}}");
}

#[test]
fn terms_work_in_regime_b_with_one_shared_table() {
    // Cross-world term identity: two independent soft facts each construct
    // f(a) in a head. Correct only if f(a) is ONE term id in every world:
    // P(s(f(a))) = 1 − 0.5·0.5 = 0.75. (Per-world throwaway tables — the old
    // panic-then-refusal — would split or crash.)
    let mut p = compile(
        "@terms.\n\
         domain node.\n\
         pred q(node): Bool.\n\
         pred t(node): Bool.\n\
         pred s(node): Bool.\n\
         0.5 :: q(a).\n\
         0.5 :: t(a).\n\
         s(f(X)) :- q(X).\n\
         s(f(X)) :- t(X).\n",
    )
    .expect("checks");
    let ans = prob_query(&mut p, "s", &[None]).expect("terms + enumeration");
    assert_eq!(ans.len(), 1);
    assert!((ans[0].1 - 0.75).abs() < 1e-12, "got {}", ans[0].1);

    // Provenance capture threads the same table: two one-leaf proofs, exact
    // WMC agrees with enumeration.
    let mut p2 = compile(
        "@terms.\n\
         domain node.\n\
         pred q(node): Bool.\n\
         pred t(node): Bool.\n\
         pred s(node): Prov.\n\
         0.5 :: q(a).\n\
         0.5 :: t(a).\n\
         s(f(X)) :- q(X).\n\
         s(f(X)) :- t(X).\n",
    )
    .expect("checks");
    let dbp = provenance(&mut p2).expect("capture over terms");
    let hits = dbp.query("s", &[None]);
    assert_eq!(hits.len(), 1);
    let (_, proofs) = &hits[0];
    assert_eq!(proofs.len(), 2, "two independent one-leaf proofs");
    let probs: Vec<f64> = p2.prob_edb.iter().map(|x| x.2).collect();
    let c = strata_k::compile_exact(proofs, probs.len()).expect("compiles");
    assert!((c.wmc(&probs) - 0.75).abs() < 1e-12);
}

#[test]
fn inputs_load_through_the_facade_with_typed_columns() {
    use strata_k::load_inputs;
    let dir = std::env::temp_dir().join(format!("strata_k_inputs_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // The value-space-split regression: an `int` column loaded from TSV must
    // be the same value as inline/JSON — reach(6) must exist.
    std::fs::write(dir.join("edges.tsv"), "5\t6\n").unwrap();
    std::fs::write(dir.join("edges.json"), "[[5, 8]]").unwrap();
    let src = concat!(
        "pred edge(int, int): Bool.\n",
        "pred reach(int): Bool.\n",
        "reach(Y) :- edge(5, Y).\n",
        "edge(5, 7).\n",
        "input edge from \"edges.tsv\".\n",
        "input edge from \"edges.json\".\n",
    );
    let (prog, d) = strata_k::parse(src);
    assert!(!d.has_errors());
    let mut checked = strata_check::check_program(&prog).expect("check");
    load_inputs(&prog, &mut checked, &dir).expect("loads");
    let db = eval(&mut checked).expect("runs");
    let reach = db.relation("reach").unwrap();
    assert_eq!(
        reach.len(),
        3,
        "6, 7, 8 — one value space across load paths"
    );

    // Empty symbol cells are refused, with the real file line (blank lines skipped).
    std::fs::write(dir.join("bad.tsv"), "a\tb\n\n\tc\n").unwrap();
    let src2 = "pred e(node, node): Bool.\ninput e from \"bad.tsv\".\n";
    let (prog2, _) = strata_k::parse(src2);
    let mut c2 = strata_check::check_program(&prog2).expect("check");
    let err = load_inputs(&prog2, &mut c2, &dir).expect_err("empty cell");
    assert!(err.contains("bad.tsv:3"), "real line number: {err}");
    assert!(err.contains("empty value"), "{err}");

    // A float in an int column is an error for every format.
    std::fs::write(dir.join("f.csv"), "1.5,2\n").unwrap();
    let src3 = "pred e(int, int): Bool.\ninput e from \"f.csv\".\n";
    let (prog3, _) = strata_k::parse(src3);
    let mut c3 = strata_check::check_program(&prog3).expect("check");
    let err = load_inputs(&prog3, &mut c3, &dir).expect_err("float in int col");
    assert!(err.contains("is `int`"), "{err}");
}

#[test]
fn second_load_inputs_fails_and_leaves_state_unchanged() {
    use strata_k::load_inputs;
    let dir = std::env::temp_dir().join(format!("strata_k_reload_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("flags.tsv"), "acme\t0.9\n").unwrap();
    let src = concat!(
        "domain firm.\n",
        "neural flag(firm) from model \"m\".\n",
        "pred investigate(firm): Bool.\n",
        "investigate(X) :- flag(X).\n",
        "input flag from \"flags.tsv\".\n",
    );
    let (prog, d) = strata_k::parse(src);
    assert!(!d.has_errors());
    let mut checked = strata_check::check_program(&prog).expect("check");
    load_inputs(&prog, &mut checked, &dir).expect("first load");
    let before = checked.prob_edb.clone();
    assert_eq!(before.len(), 1);
    // A second load would double the soft fact and shift P(0.9) to 0.99 — refuse
    // instead, and leave prob_edb exactly as the first load left it.
    let err = load_inputs(&prog, &mut checked, &dir).expect_err("second load");
    assert!(err.contains("already called"), "{err}");
    assert_eq!(
        checked.prob_edb, before,
        "failed reload left nothing behind"
    );
}

#[test]
fn wrong_arity_model_output_is_a_typed_error() {
    struct Fat;
    impl Model for Fat {
        fn name(&self) -> &str {
            "risk_gnn"
        }
        fn soft_facts(&self, dict: &mut SymbolDict) -> Vec<(String, Tuple, f64)> {
            let a = GroundVal::Sym(dict.intern("acme"));
            vec![("flag".to_string(), vec![a, a], 0.9)]
        }
    }
    let mut p = compile(NEURAL_SRC).expect("checks");
    assert!(matches!(
        attach_models(&mut p, &[&Fat]),
        Err(ModelError::WrongArity {
            expected: 1,
            got: 2,
            ..
        })
    ));
}

#[test]
fn second_attach_raises_instead_of_duplicating() {
    struct One;
    impl Model for One {
        fn name(&self) -> &str {
            "risk_gnn"
        }
        fn soft_facts(&self, dict: &mut SymbolDict) -> Vec<(String, Tuple, f64)> {
            vec![(
                "flag".to_string(),
                vec![GroundVal::Sym(dict.intern("acme"))],
                0.9,
            )]
        }
    }
    let mut p = compile(NEURAL_SRC).expect("checks");
    attach_models(&mut p, &[&One]).expect("first attach");
    let before = p.prob_edb.clone();
    // The round trip an external caller will write: attach twice by mistake.
    // Silently doubling prob_edb here would shift every marginal (0.9 -> 0.99).
    assert!(matches!(
        attach_models(&mut p, &[&One]),
        Err(ModelError::AlreadyAttached)
    ));
    assert_eq!(
        p.prob_edb, before,
        "failed second attach left nothing behind"
    );
}

// --- the in-process neural boundary -------------------------------------------

/// A stand-in production model: scores firms from an in-memory feature table.
/// The point is the *wiring* — its forward pass runs inside the evaluation
/// setup, and the program never contains a pasted `0.9 :: flag(...)` line.
struct RiskModel {
    scores: Vec<(&'static str, f64)>,
}

impl Model for RiskModel {
    fn name(&self) -> &str {
        "risk_gnn"
    }
    fn soft_facts(&self, dict: &mut SymbolDict) -> Vec<(String, Tuple, f64)> {
        self.scores
            .iter()
            .map(|(firm, p)| {
                (
                    "flag".to_string(),
                    vec![GroundVal::Sym(dict.intern(firm))],
                    *p,
                )
            })
            .collect()
    }
}

const NEURAL_SRC: &str = "\
domain firm.
neural flag(firm) from model \"risk_gnn\".
pred investigate(firm): Prov.
investigate(X) :- flag(X).
";

#[test]
fn in_process_model_computes_the_soft_facts() {
    let mut p = compile(NEURAL_SRC).expect("checks");
    assert!(p.prob_edb.is_empty(), "nothing pasted in the source");
    let model = RiskModel {
        scores: vec![("acme", 0.9), ("globex", 0.2)],
    };
    attach_models(&mut p, &[&model]).expect("attaches");
    assert_eq!(p.prob_edb.len(), 2, "the forward pass supplied the facts");

    let pat = [Some(sym(&p.dict, "acme"))];
    let ans = grad_query(&mut p, "investigate", &pat).expect("grad");
    assert_eq!(ans.len(), 1);
    assert!(
        (ans[0].1 - 0.9).abs() < 1e-12,
        "marginal = model confidence"
    );
    assert!(
        (ans[0].2[0] - 1.0).abs() < 1e-12,
        "∂/∂flag(acme) = 1 → backprop"
    );
}

#[test]
fn model_wiring_errors_are_typed() {
    // Missing model.
    let mut p = compile(NEURAL_SRC).expect("checks");
    assert!(matches!(
        attach_models(&mut p, &[]),
        Err(ModelError::MissingModel(m)) if m == "risk_gnn"
    ));

    // A model speaking for a predicate it wasn't declared against.
    struct Rogue;
    impl Model for Rogue {
        fn name(&self) -> &str {
            "risk_gnn"
        }
        fn soft_facts(&self, dict: &mut SymbolDict) -> Vec<(String, Tuple, f64)> {
            vec![(
                "investigate".to_string(),
                vec![GroundVal::Sym(dict.intern("acme"))],
                0.5,
            )]
        }
    }
    let mut p = compile(NEURAL_SRC).expect("checks");
    assert!(matches!(
        attach_models(&mut p, &[&Rogue]),
        Err(ModelError::WrongPredicate { pred, .. }) if pred == "investigate"
    ));

    // A probability outside [0, 1].
    let mut p = compile(NEURAL_SRC).expect("checks");
    let bad = RiskModel {
        scores: vec![("acme", 1.5)],
    };
    assert!(matches!(
        attach_models(&mut p, &[&bad]),
        Err(ModelError::BadProbability { .. })
    ));
}
