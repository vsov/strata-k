//! Roundtrip: parse → print → parse is a fixpoint; formatting is idempotent. [FRONT-8/9, D12]

use strata_front::{format, is_formatted, parse, print_program};

const SAMPLES: &[&str] = &[
    "domain node.\npred edge(node, node): Bool.\npred path(node, node): Bool.\npath(X, Z) :- edge(X, Y), path(Y, Z).\n",
    "pred reach(node): Bool.\npred unreach(node): Bool.\nunreach(X) :- node(X), not reach(X).\n",
    "pred outdeg(node, int): Bool.\noutdeg(X, count<Y>) :- edge(X, Y).\n",
    "pred edge(node, node): Trop partial.\n5 :: edge(a, b).\ninput edge from \"edges.tsv\".\n",
    "? path(a, X).\n",
    "pred controls(firm, firm): Prov.\n0.9 :: owns(a, b).\n?prob controls(a, X).\n",
    "pred reach(node, node): Prov_k(5).\n?grad reach(a, X).\n",
];

#[test]
fn bare_prov_k_formats_to_its_default_bound() {
    // `Prov_k` with no bound is `Prov_k(3)`; fmt makes the k explicit.
    let (p, d) = parse("pred r(node): Prov_k.\n");
    assert!(!d.has_errors());
    let printed = print_program(&p);
    assert!(printed.contains("Prov_k(3)"), "{printed}");
    let (p2, _) = parse(&printed);
    assert_eq!(p, p2);
}

#[test]
fn prov_k_rejects_a_zero_bound() {
    let (_, d) = parse("pred r(node): Prov_k(0).\n");
    assert!(d.has_errors(), "Prov_k(0) must not parse");
}

#[test]
fn parse_print_parse_is_a_fixpoint() {
    for src in SAMPLES {
        let (p1, d1) = parse(src);
        assert!(!d1.has_errors(), "{}", d1.render_text(src));
        let printed = print_program(&p1);
        let (p2, d2) = parse(&printed);
        assert!(
            !d2.has_errors(),
            "reparse failed:\n{}\n{}",
            printed,
            d2.render_text(&printed)
        );
        assert_eq!(p1, p2, "parse→print→parse changed the IR for:\n{src}");
    }
}

const COMMENTED: &str = "\
% leading comment on edge
pred edge(node, node): Bool.
% a rule follows
path(X, Y) :- edge(X, Y).
";

#[test]
fn comments_survive_roundtrip() {
    let (p1, d1) = parse(COMMENTED);
    assert!(!d1.has_errors(), "{}", d1.render_text(COMMENTED));
    let printed = print_program(&p1);
    // both comments are re-emitted verbatim
    assert!(printed.contains("% leading comment on edge"), "{printed}");
    assert!(printed.contains("% a rule follows"), "{printed}");
    // and the roundtrip is a fixpoint
    let (p2, _) = parse(&printed);
    assert_eq!(p1, p2);
    assert_eq!(
        format(COMMENTED).unwrap(),
        format(&format(COMMENTED).unwrap()).unwrap()
    );
}

#[test]
fn blank_lines_between_blocks_survive_roundtrip() {
    let src = "\
pred edge(node, node): Bool.

pred path(node, node): Bool.
path(X, Y) :- edge(X, Y).
";
    // fmt keeps the blank line between the two declarations, and is a fixpoint.
    let once = format(src).unwrap();
    assert!(
        once.contains("Bool.\n\npred path"),
        "blank line not preserved:\n{once}"
    );
    assert_eq!(once, format(&once).unwrap(), "format must be idempotent");
    let (p1, _) = parse(src);
    let (p2, _) = parse(&once);
    assert_eq!(p1, p2);
}

#[test]
fn comments_do_not_affect_semantic_equality() {
    // IR-5: trivia is excluded from equality.
    let (with, _) = parse("% a note\np(a).\n");
    let (without, _) = parse("p(a).\n");
    assert_eq!(with, without);
}

#[test]
fn formatting_is_idempotent() {
    for src in SAMPLES {
        let once = format(src).expect("format");
        let twice = format(&once).expect("format twice");
        assert_eq!(once, twice, "format not idempotent for:\n{src}");
        assert!(
            is_formatted(&once),
            "formatted output must be canonical:\n{once}"
        );
    }
}
