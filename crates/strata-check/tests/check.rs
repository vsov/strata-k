//! Checker tests: stratification, lowering, and the error paths. [CHECK-2/3/10/12/13]

use strata_check::{check_program, codes, Checked};
use strata_front::parse;
use strata_ir::core::Semiring;
use strata_ir::high::program::{atom, var, ItemKind, Literal, PredDecl, Rule};
use strata_ir::high::sig::{Annotation, ArgType, Effects, Signature};
use strata_ir::high::{Item, Program};

/// Hand-build a single-argument predicate declaration with a given annotation.
fn pred1(name: &str, ann: Annotation) -> Item {
    Item::new(ItemKind::Predicate(PredDecl {
        name: name.into(),
        sig: Signature {
            args: vec![ArgType::Domain {
                name: "node".into(),
            }],
            annotation: ann,
            effects: Effects::default(),
        },
        neural: None,
    }))
}

fn err_codes(prog: &Program) -> Vec<u16> {
    check_program(prog)
        .err()
        .map(|d| d.items().iter().map(|x| x.code.0).collect())
        .unwrap_or_default()
}

fn check(src: &str) -> Result<Checked, Vec<u16>> {
    let (prog, diags) = parse(src);
    assert!(
        !diags.has_errors(),
        "parse errors:\n{}",
        diags.render_text(src)
    );
    check_program(&prog).map_err(|d| d.items().iter().map(|x| x.code.0).collect())
}

#[test]
fn transitive_closure_lowers() {
    let src = "\
pred edge(node, node): Bool.
pred path(node, node): Bool.
path(X, Y) :- edge(X, Y).
path(X, Z) :- edge(X, Y), path(Y, Z).
edge(a, b).
edge(b, c).
";
    let c = check(src).expect("check ok");
    assert_eq!(c.core.num_strata, 1);
    assert_eq!(c.core.predicates.len(), 2);
    assert_eq!(c.core.rules.len(), 2);
    assert_eq!(c.edb.len(), 2);
    // constants interned
    assert_eq!(c.dict.len(), 3); // a, b, c
                                 // path is Bool at stratum 0
    let path = c.core.predicates.iter().find(|p| p.name == "path").unwrap();
    assert_eq!(path.semiring, Semiring::Bool);
    assert_eq!(path.stratum, 0);
    // the recursive rule got 3 variable slots (X, Y, Z)
    let rec = c.core.rules.iter().find(|r| r.body.len() == 2).unwrap();
    assert_eq!(rec.var_count, 3);
}

#[test]
fn stratified_negation_gets_two_strata() {
    let src = "\
pred node(node): Bool.
pred reach(node): Bool.
pred unreach(node): Bool.
reach(a).
unreach(X) :- node(X), not reach(X).
";
    let c = check(src).expect("check ok");
    assert_eq!(c.core.num_strata, 2);
    let unreach = c
        .core
        .predicates
        .iter()
        .find(|p| p.name == "unreach")
        .unwrap();
    let reach = c
        .core
        .predicates
        .iter()
        .find(|p| p.name == "reach")
        .unwrap();
    assert!(
        unreach.stratum > reach.stratum,
        "unreach must be above reach"
    );
}

#[test]
fn negation_cycle_is_unstratifiable() {
    let src = "\
pred q(node): Bool.
pred p(node): Bool.
p(X) :- q(X), not p(X).
";
    let codes = check(src).unwrap_err();
    assert!(codes.contains(&codes::UNSTRATIFIABLE.0), "got {codes:?}");
}

#[test]
fn undeclared_predicate_is_rejected() {
    // `edge` is never declared; X occurs 3× so it is not a singleton.
    let src = "\
pred p(node): Bool.
p(X) :- edge(X, X).
";
    let codes = check(src).unwrap_err();
    assert!(codes.contains(&codes::UNDECLARED_PRED.0), "got {codes:?}");
}

#[test]
fn unbound_head_variable_is_rejected() {
    // X occurs in the head and a negated literal (not a singleton) but never in
    // a positive body literal → range-restriction violation.
    let src = "\
pred p(node): Bool.
pred q(node): Bool.
pred r(node): Bool.
q(a).
p(X) :- q(a), not r(X).
";
    let codes = check(src).unwrap_err();
    assert!(
        codes.contains(&codes::NOT_RANGE_RESTRICTED.0),
        "got {codes:?}"
    );
}

#[test]
fn diagnostics_carry_a_source_span() {
    let src = "pred path(node, node): Bool.\npath(X, X) :- edge(X, X).\n";
    let (prog, _) = parse(src);
    let diags = check_program(&prog).unwrap_err();
    let d = diags
        .items()
        .iter()
        .find(|d| d.code == codes::UNDECLARED_PRED)
        .unwrap();
    assert!(
        !d.primary.is_zero(),
        "check diagnostic should point at source, got {:?}",
        d.primary
    );
}

#[test]
fn recursive_prov_is_a_table_2_4_forbidden_cell() {
    // p: Prov, and p(X) :- p(X). — recursive probabilistic provenance is
    // forbidden exactly; the diagnostic must suggest Prov_k.
    let prog = Program::new(vec![
        pred1("p", Annotation::Prov),
        Item::new(ItemKind::Rule(Rule {
            head: atom("p", vec![var("X")]),
            body: vec![Literal::Pos(atom("p", vec![var("X")]))],
        })),
    ]);
    let diags = check_program(&prog).unwrap_err();
    let d = diags
        .items()
        .iter()
        .find(|d| d.code == codes::TABLE_2_4_FORBIDDEN)
        .unwrap();
    assert!(
        d.message.contains("Prov_k"),
        "should suggest the nearest alternative: {}",
        d.message
    );
}

#[test]
fn nonrecursive_prov_lowers_to_bool_core() {
    // p: Prov, non-recursive (p(X) :- q(X), q: Bool) → режим B by capture +
    // compilation; Core-IR carries it set-wise as Bool, the annotation map
    // routes it to the provenance evaluator.
    let prog = Program::new(vec![
        pred1("p", Annotation::Prov),
        pred1("q", Annotation::Bool),
        Item::new(ItemKind::Rule(Rule {
            head: atom("p", vec![var("X")]),
            body: vec![Literal::Pos(atom("q", vec![var("X")]))],
        })),
    ]);
    let c = check_program(&prog).expect("non-recursive Prov checks");
    let p = c.core.predicates.iter().find(|p| p.name == "p").unwrap();
    assert_eq!(p.semiring, Semiring::Bool);
    assert_eq!(c.annotations["p"], Annotation::Prov);
    assert_eq!(c.core.rules.len(), 1, "the Prov rule is lowered");
}

#[test]
fn recursive_prov_k_is_allowed() {
    // Prov_k exists precisely for the recursive soft case (spec 2.2).
    let prog = Program::new(vec![
        pred1("p", Annotation::ProvK { k: 3 }),
        Item::new(ItemKind::Rule(Rule {
            head: atom("p", vec![var("X")]),
            body: vec![Literal::Pos(atom("p", vec![var("X")]))],
        })),
    ]);
    let c = check_program(&prog).expect("recursive Prov_k checks");
    assert_eq!(c.annotations["p"], Annotation::ProvK { k: 3 });
}

#[test]
fn prov_k_zero_keeps_no_proofs() {
    let prog = Program::new(vec![pred1("p", Annotation::ProvK { k: 0 })]);
    let codes = err_codes(&prog);
    assert!(codes.contains(&codes::NOT_EXECUTABLE.0), "got {codes:?}");
}

#[test]
fn soft_cannot_launder_into_bool() {
    // Prov body into a Bool head would erase the taint — forbidden.
    let prog = Program::new(vec![
        pred1("soft", Annotation::Prov),
        pred1("hard", Annotation::Bool),
        Item::new(ItemKind::Rule(Rule {
            head: atom("hard", vec![var("X")]),
            body: vec![Literal::Pos(atom("soft", vec![var("X")]))],
        })),
    ]);
    let codes = err_codes(&prog);
    assert!(codes.contains(&codes::SEMIRING_CONFLICT.0), "got {codes:?}");
}

#[test]
fn trop_and_prov_are_incomparable() {
    // No homomorphism either way (spec 1.7): Trop body into a Prov head errors.
    let prog = Program::new(vec![
        pred1("w", Annotation::Trop),
        pred1("p", Annotation::Prov),
        Item::new(ItemKind::Rule(Rule {
            head: atom("p", vec![var("X")]),
            body: vec![Literal::Pos(atom("w", vec![var("X")]))],
        })),
    ]);
    let codes = err_codes(&prog);
    assert!(codes.contains(&codes::SEMIRING_CONFLICT.0), "got {codes:?}");
}

#[test]
fn bool_flows_into_prov() {
    // Bool ⊑ Prov: certain evidence may support a provenance conclusion.
    let src = "\
pred q(node): Bool.
pred p(node): Prov.
p(X) :- q(X).
q(a).
";
    check(src).expect("Bool → Prov is a lattice edge");
}

#[test]
fn fact_annotation_contract_e1009() {
    // Every cell of the fact `::` × declared-annotation table (E1009).
    let bad = [
        // int weight on non-Trop: the silently-accepted-typos hole.
        "pred e(node, node): Bool.\n5 :: e(a, b).\n",
        "pred p(node): Prov.\n5 :: p(a).\n",
        "pred r(node): Prov_k(2).\n5 :: r(a).\n",
        // probability on Trop.
        "pred w(node, node): Trop.\n0.5 :: w(a, b).\n",
        // probability outside [0, 1].
        "pred q(node): Bool.\n1.5 :: q(a).\n",
        // bare fact on Trop: would panic the tropical fixpoint downstream.
        "pred w(node, node): Trop.\nw(a, b).\n",
    ];
    for src in bad {
        let codes = check(src).expect_err(&format!("must reject:\n{src}"));
        assert!(
            codes.contains(&codes::FACT_ANNOTATION_MISMATCH.0),
            "expected E1009 for:\n{src}\ngot {codes:?}"
        );
    }
    // The well-typed cells still lower.
    let good = [
        "pred e(node, node): Bool.\ne(a, b).\n",
        "pred e(node, node): Bool.\n0.5 :: e(a, b).\n",
        "pred w(node, node): Trop.\n5 :: w(a, b).\n",
        "pred p(node): Prov.\np(a).\n",
        "pred p(node): Prov.\n0.9 :: p(a).\n",
    ];
    for src in good {
        check(src).unwrap_or_else(|e| panic!("must accept:\n{src}\ngot {e:?}"));
    }
}

#[test]
fn asp_refuses_what_it_cannot_mean_e1011() {
    use strata_check::check_asp_declarations;
    // Every construct the @asp runner used to silently drop is now refused by
    // name: `::` annotations, non-ground/compound facts, queries, `input`,
    // and `neural` declarations.
    let cases: &[(&str, u16)] = &[
        (
            "@asp.\npred a(): Bool.\n0.5 :: a().\n",
            codes::ASP_UNSUPPORTED.0,
        ),
        (
            "@asp.\npred a(): Bool.\n5 :: a().\n",
            codes::ASP_UNSUPPORTED.0,
        ),
        (
            "@asp.\npred p(node): Bool.\np(X).\n",
            codes::NON_GROUND_FACT.0,
        ),
        (
            "@asp.\npred p(node): Bool.\np(box(a)).\n",
            codes::ASP_UNSUPPORTED.0,
        ),
        (
            "@asp.\npred a(): Bool.\na().\n? a().\n",
            codes::ASP_UNSUPPORTED.0,
        ),
        (
            "@asp.\npred e(node, node): Bool.\ninput e from \"edges.tsv\".\n",
            codes::ASP_UNSUPPORTED.0,
        ),
        (
            "@asp.\ndomain firm.\nneural f(firm) from model \"m\".\n0.9 :: f(acme).\n",
            codes::ASP_UNSUPPORTED.0,
        ),
    ];
    for (src, want) in cases {
        let (prog, d) = parse(src);
        assert!(!d.has_errors(), "parse errors for:\n{src}");
        let diags = check_asp_declarations(&prog).expect_err(&format!("must reject:\n{src}"));
        let got: Vec<u16> = diags.items().iter().map(|x| x.code.0).collect();
        assert!(
            got.contains(want),
            "expected E{want} for:\n{src}\ngot {got:?}"
        );
    }
}

#[test]
fn conflicting_redeclaration_is_rejected_e1012() {
    // Last-wins redeclaration would make every downstream check order-dependent.
    let src = "\
pred e(node, node): Bool.
pred e(node, node): Trop.
5 :: e(a, b).
";
    let codes_got = check(src).unwrap_err();
    assert!(
        codes_got.contains(&codes::CONFLICTING_DECLARATION.0),
        "got {codes_got:?}"
    );
    // An identical redeclaration is harmless.
    check("pred e(node): Bool.\npred e(node): Bool.\ne(a).\n").expect("identical redecl ok");
    // And @asp checks arity conflicts too.
    let (prog, _) = parse("@asp.\npred a(node): Bool.\npred a(node, node): Bool.\na(x).\n");
    let diags = strata_check::check_asp_declarations(&prog).unwrap_err();
    let got: Vec<u16> = diags.items().iter().map(|x| x.code.0).collect();
    assert!(
        got.contains(&codes::CONFLICTING_DECLARATION.0),
        "got {got:?}"
    );
}

#[test]
fn asp_requires_declarations_too() {
    use strata_check::check_asp_declarations;
    // Undeclared predicate inside @asp: the signature promise is global.
    let (prog, d) = parse("@asp.\na() :- not b().\n");
    assert!(!d.has_errors());
    let diags = check_asp_declarations(&prog).unwrap_err();
    let codes: Vec<u16> = diags.items().iter().map(|x| x.code.0).collect();
    assert!(codes.contains(&codes::UNDECLARED_PRED.0), "got {codes:?}");

    // Arity mismatch is caught as well.
    let (prog, _) = parse("@asp.\npred a(node): Bool.\na() :- not a(x).\n");
    let diags = check_asp_declarations(&prog).unwrap_err();
    let codes: Vec<u16> = diags.items().iter().map(|x| x.code.0).collect();
    assert!(codes.contains(&codes::ARITY_MISMATCH.0), "got {codes:?}");

    // A fully declared ASP program passes (no stratification demanded).
    let (prog, _) =
        parse("@asp.\npred a(): Bool.\npred b(): Bool.\na() :- not b().\nb() :- not a().\n");
    check_asp_declarations(&prog).expect("declared @asp checks");
}

#[test]
fn neural_weighted_fact_is_rejected() {
    // `5 :: flag(...)` on a neural predicate: an integer weight on a Bool-side
    // predicate is E1009 (neural facts must be probabilities).
    let src = "\
domain firm.
neural flag(firm) from model \"m\".
5 :: flag(acme).
";
    let codes = check(src).expect_err("weighted neural fact must fail");
    assert!(
        codes.contains(&codes::FACT_ANNOTATION_MISMATCH.0),
        "got {codes:?}"
    );
}

#[test]
fn trop_into_bool_is_a_semiring_conflict() {
    let src = "\
pred e(node, node): Trop.
pred p(node, node): Bool.
p(X, Y) :- e(X, Y).
";
    let codes = check(src).unwrap_err();
    assert!(codes.contains(&codes::SEMIRING_CONFLICT.0), "got {codes:?}");
}
