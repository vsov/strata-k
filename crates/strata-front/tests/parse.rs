//! Parser tests: surface → High-IR, diagnostics, recovery. [FRONT-4/5/6/7]

use strata_front::{parse, print_program};
use strata_ir::high::program::{
    atom, cst, var, DomainDecl, Fact, ItemKind, Literal, PredDecl, Rule,
};
use strata_ir::high::sig::{Annotation, ArgType, Effects, Signature};
use strata_ir::high::{Item, Program};

const TC: &str = "\
domain node.
pred edge(node, node): Bool.
pred path(node, node): Bool.
path(X, Y) :- edge(X, Y).
path(X, Z) :- edge(X, Y), path(Y, Z).
edge(a, b).
edge(b, c).
";

#[test]
fn transitive_closure_parses_to_expected_ir() {
    let (prog, diags) = parse(TC);
    assert!(!diags.has_errors(), "{}", diags.render_text(TC));

    let dom = |n: &str| ArgType::Domain { name: n.into() };
    let bool_pred = |n: &str| {
        Item::new(ItemKind::Predicate(PredDecl {
            name: n.into(),
            sig: Signature {
                args: vec![dom("node"), dom("node")],
                annotation: Annotation::Bool,
                effects: Effects::default(),
            },
            neural: None,
        }))
    };
    let expected = Program::new(vec![
        Item::new(ItemKind::Domain(DomainDecl {
            name: "node".into(),
        })),
        bool_pred("edge"),
        bool_pred("path"),
        Item::new(ItemKind::Rule(Rule {
            head: atom("path", vec![var("X"), var("Y")]),
            body: vec![Literal::Pos(atom("edge", vec![var("X"), var("Y")]))],
        })),
        Item::new(ItemKind::Rule(Rule {
            head: atom("path", vec![var("X"), var("Z")]),
            body: vec![
                Literal::Pos(atom("edge", vec![var("X"), var("Y")])),
                Literal::Pos(atom("path", vec![var("Y"), var("Z")])),
            ],
        })),
        Item::new(ItemKind::Fact(Fact {
            atom: atom("edge", vec![cst("a"), cst("b")]),
            weight: None,
            prob: None,
        })),
        Item::new(ItemKind::Fact(Fact {
            atom: atom("edge", vec![cst("b"), cst("c")]),
            weight: None,
            prob: None,
        })),
    ]);

    assert_eq!(prog, expected);
}

#[test]
fn negation_and_aggregate_parse() {
    let src = "\
pred node(node): Bool.
pred reach(node): Bool.
pred unreach(node): Bool.
pred outdeg(node, int): Bool.
unreach(X) :- node(X), not reach(X).
outdeg(X, count<Y>) :- edge(X, Y).
";
    let (prog, diags) = parse(src);
    assert!(!diags.has_errors(), "{}", diags.render_text(src));
    // the negated body literal survived
    let has_neg = prog.items.iter().any(|i| {
        matches!(&i.node,
        ItemKind::Rule(r) if r.body.iter().any(|l| matches!(l, Literal::Neg(_))))
    });
    assert!(has_neg);
}

#[test]
fn singleton_variable_is_an_error_with_fix() {
    // Y appears once → error; the fix renames it `_Y`.
    let src = "p(X) :- edge(X, Y).\n";
    let (_prog, diags) = parse(src);
    assert!(diags.has_errors());
    let d = diags
        .items()
        .iter()
        .find(|d| d.code == strata_front::codes::SINGLETON_VAR)
        .unwrap();
    assert_eq!(d.fixes.len(), 1);
    assert_eq!(d.fixes[0].replacement, "_Y");
}

#[test]
fn underscore_variable_is_not_a_singleton() {
    let src = "p(X) :- edge(X, _Y).\n";
    let (_prog, diags) = parse(src);
    assert!(!diags.has_errors(), "{}", diags.render_text(src));
}

#[test]
fn probabilistic_fact_parses_and_is_executable() {
    // Phase 4: probabilistic facts are no longer gated — they parse cleanly and
    // carry their probability.
    let src = "0.87 :: edge(a, b).\n";
    let (prog, diags) = parse(src);
    assert!(!diags.has_errors(), "{}", diags.render_text(src));
    assert_eq!(prog.items.len(), 1);
    let ItemKind::Fact(f) = &prog.items[0].node else {
        panic!("expected a fact")
    };
    assert_eq!(f.prob, Some(0.87));
}

#[test]
fn recovery_keeps_later_clauses() {
    // First clause is missing its `)`; the second must still parse.
    let src = "p(a, b.\nq(c).\n";
    let (prog, diags) = parse(src);
    assert!(diags.has_errors());
    assert!(
        prog.items
            .iter()
            .any(|i| matches!(&i.node, ItemKind::Fact(f) if f.atom.pred == "q")),
        "recovery should have parsed q(c)."
    );
}

#[test]
fn printer_reparses_to_the_same_ir() {
    let (prog, diags) = parse(TC);
    assert!(!diags.has_errors());
    let printed = print_program(&prog);
    let (prog2, diags2) = parse(&printed);
    assert!(!diags2.has_errors(), "{}", diags2.render_text(&printed));
    assert_eq!(prog, prog2, "parse(print(ir)) must equal ir");
}
