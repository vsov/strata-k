//! Checker tests: stratification, lowering, and the error paths. [CHECK-2/3/10/12/13]

use strata_check::{check_program, codes, Checked};
use strata_front::parse;
use strata_ir::core::Semiring;
use strata_ir::high::program::{atom, var, ItemKind, Literal, PredDecl, Rule};
use strata_ir::high::sig::{Annotation, ArgType, Effects, Signature};
use strata_ir::high::{Item, Program};

/// Hand-build a single-argument predicate declaration with a given annotation
/// (bypasses the parser, which rejects Prov/Prov_k at the surface).
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
fn nonrecursive_prov_is_not_executable_in_phase0() {
    // p: Prov, non-recursive (p(X) :- q(X), q: Bool) → режим B, not implemented.
    let prog = Program::new(vec![
        pred1("p", Annotation::Prov),
        pred1("q", Annotation::Bool),
        Item::new(ItemKind::Rule(Rule {
            head: atom("p", vec![var("X")]),
            body: vec![Literal::Pos(atom("q", vec![var("X")]))],
        })),
    ]);
    let codes = err_codes(&prog);
    assert!(codes.contains(&codes::NOT_EXECUTABLE.0), "got {codes:?}");
    assert!(
        !codes.contains(&codes::TABLE_2_4_FORBIDDEN.0),
        "non-recursive Prov is not the forbidden cell"
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
