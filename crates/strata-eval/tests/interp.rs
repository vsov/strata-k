//! Slice 2 tracer: the reference interpreter runs on HAND-WRITTEN Core-IR — no
//! parser and no checker exist yet (D15 item 2). [EVAL-1/2/3]
//!
//! Transitive closure (Bool) matches the hand-computed closure; SSSP (Trop)
//! matches hand-computed Dijkstra distances; i64 overflow surfaces as an error.

use std::collections::BTreeSet;

use strata_eval::{run, run_semi_naive, Ann, Db, EvalError, GroundVal};
use strata_ir::core::{CoreAtom, CoreLiteral, CorePred, CoreProgram, CoreRule, CoreTerm, Semiring};
use strata_ir::dict::{SymbolDict, SymbolId};
use strata_ir::high::program::AggOp;
use strata_ir::trop::Weight;

/// Run BOTH engines and assert bit-identical databases (EVAL-8 / I5), returning
/// the naive result for further assertions. Every success-path test flows
/// through here, so semi-naive is cross-checked on every program.
fn agree(prog: &CoreProgram, edb: &[(&str, Vec<GroundVal>, Ann)]) -> Db {
    let naive = run(prog, edb).expect("naive eval");
    let semi = run_semi_naive(prog, edb).expect("semi-naive eval");
    assert_eq!(naive, semi, "naive and semi-naive must agree bit-for-bit");
    naive
}

// --- tiny builders to keep the hand-written Core-IR readable -----------------

fn v(slot: u32) -> CoreTerm {
    CoreTerm::Var { slot }
}
fn pos(pred: &str, args: Vec<CoreTerm>) -> CoreLiteral {
    CoreLiteral::Pos(CoreAtom {
        pred: pred.into(),
        args,
    })
}
fn head(pred: &str, args: Vec<CoreTerm>) -> CoreAtom {
    CoreAtom {
        pred: pred.into(),
        args,
    }
}
fn pred(name: &str, sem: Semiring) -> CorePred {
    CorePred {
        name: name.into(),
        arity: 2,
        semiring: sem,
        stratum: 0,
    }
}
fn rule(h: CoreAtom, body: Vec<CoreLiteral>, var_count: u32) -> CoreRule {
    CoreRule {
        head: h,
        body,
        stratum: 0,
        var_count,
        neg_weight_cycle_check: false,
    }
}

#[test]
fn transitive_closure_bool() {
    // path(X,Y) :- edge(X,Y).
    // path(X,Z) :- edge(X,Y), path(Y,Z).
    let prog = CoreProgram {
        predicates: vec![pred("edge", Semiring::Bool), pred("path", Semiring::Bool)],
        rules: vec![
            rule(
                head("path", vec![v(0), v(1)]),
                vec![pos("edge", vec![v(0), v(1)])],
                2,
            ),
            rule(
                head("path", vec![v(0), v(2)]),
                vec![pos("edge", vec![v(0), v(1)]), pos("path", vec![v(1), v(2)])],
                3,
            ),
        ],
        num_strata: 1,
    };

    let mut dict = SymbolDict::new();
    let mut sym = |s: &str| GroundVal::Sym(dict.intern(s));
    // chain a -> b -> c -> d
    let edb = vec![
        ("edge", vec![sym("a"), sym("b")], Ann::Unit),
        ("edge", vec![sym("b"), sym("c")], Ann::Unit),
        ("edge", vec![sym("c"), sym("d")], Ann::Unit),
    ];
    let db = agree(&prog, &edb);

    let got: BTreeSet<(SymbolId, SymbolId)> = db
        .relation("path")
        .unwrap()
        .rows
        .keys()
        .map(|t| match (t[0], t[1]) {
            (GroundVal::Sym(a), GroundVal::Sym(b)) => (a, b),
            _ => panic!("expected symbols"),
        })
        .collect();

    let s = |name: &str| dict.get(name).unwrap();
    let expected: BTreeSet<(SymbolId, SymbolId)> = [
        (s("a"), s("b")),
        (s("b"), s("c")),
        (s("c"), s("d")),
        (s("a"), s("c")),
        (s("b"), s("d")),
        (s("a"), s("d")),
    ]
    .into_iter()
    .collect();

    assert_eq!(got, expected, "transitive closure of a->b->c->d");
}

#[test]
fn sssp_trop_min_plus() {
    // reach(X,Y) :- edge(X,Y).
    // reach(X,Z) :- edge(X,Y), reach(Y,Z).
    let prog = CoreProgram {
        predicates: vec![pred("edge", Semiring::Trop), pred("reach", Semiring::Trop)],
        rules: vec![
            rule(
                head("reach", vec![v(0), v(1)]),
                vec![pos("edge", vec![v(0), v(1)])],
                2,
            ),
            rule(
                head("reach", vec![v(0), v(2)]),
                vec![
                    pos("edge", vec![v(0), v(1)]),
                    pos("reach", vec![v(1), v(2)]),
                ],
                3,
            ),
        ],
        num_strata: 1,
    };

    let mut dict = SymbolDict::new();
    let w = |n: i64| Ann::W(Weight::Finite(n));
    let mut sym = |s: &str| GroundVal::Sym(dict.intern(s));
    // a->b (2), b->c (3), a->c (10): shortest a..c is 5 via b, not the direct 10.
    let edb = vec![
        ("edge", vec![sym("a"), sym("b")], w(2)),
        ("edge", vec![sym("b"), sym("c")], w(3)),
        ("edge", vec![sym("a"), sym("c")], w(10)),
    ];
    let db = agree(&prog, &edb);

    let weight_of = |from: &str, to: &str| -> Option<Weight> {
        let t = vec![
            GroundVal::Sym(dict.get(from)?),
            GroundVal::Sym(dict.get(to)?),
        ];
        db.relation("reach").unwrap().rows.get(&t).map(|a| match a {
            Ann::W(w) => *w,
            Ann::Unit => panic!("trop relation had Unit annotation"),
        })
    };

    assert_eq!(weight_of("a", "b"), Some(Weight::Finite(2)));
    assert_eq!(weight_of("b", "c"), Some(Weight::Finite(3)));
    assert_eq!(
        weight_of("a", "c"),
        Some(Weight::Finite(5)),
        "min(10, 2+3) = 5"
    );
}

#[test]
fn trop_overflow_is_error_not_wrap() {
    // reach(X,Z) :- edge(X,Y), reach(Y,Z). with weights near i64::MAX.
    let prog = CoreProgram {
        predicates: vec![pred("edge", Semiring::Trop), pred("reach", Semiring::Trop)],
        rules: vec![
            rule(
                head("reach", vec![v(0), v(1)]),
                vec![pos("edge", vec![v(0), v(1)])],
                2,
            ),
            rule(
                head("reach", vec![v(0), v(2)]),
                vec![
                    pos("edge", vec![v(0), v(1)]),
                    pos("reach", vec![v(1), v(2)]),
                ],
                3,
            ),
        ],
        num_strata: 1,
    };
    let mut dict = SymbolDict::new();
    let mut sym = |s: &str| GroundVal::Sym(dict.intern(s));
    let big = Ann::W(Weight::Finite(i64::MAX));
    let edb = vec![
        ("edge", vec![sym("a"), sym("b")], big),
        ("edge", vec![sym("b"), sym("c")], Ann::W(Weight::Finite(1))),
    ];
    let err = run(&prog, &edb).unwrap_err();
    assert!(matches!(err, EvalError::Overflow(_)), "got {err:?}");
    assert!(matches!(
        run_semi_naive(&prog, &edb).unwrap_err(),
        EvalError::Overflow(_)
    ));
}

// --- Slice 3 helpers ---------------------------------------------------------

fn predn(name: &str, arity: u32, sem: Semiring, stratum: u32) -> CorePred {
    CorePred {
        name: name.into(),
        arity,
        semiring: sem,
        stratum,
    }
}
fn rulek(h: CoreAtom, body: Vec<CoreLiteral>, var_count: u32, stratum: u32) -> CoreRule {
    CoreRule {
        head: h,
        body,
        stratum,
        var_count,
        neg_weight_cycle_check: false,
    }
}
fn neg(pred: &str, args: Vec<CoreTerm>) -> CoreLiteral {
    CoreLiteral::Neg(CoreAtom {
        pred: pred.into(),
        args,
    })
}
fn agg(op: AggOp, slot: u32) -> CoreTerm {
    CoreTerm::Agg { op, slot }
}

// --- EVAL-4/5: stratified negation -------------------------------------------

#[test]
fn stratified_negation_complement() {
    // reach(Y) :- edge(X,Y), reach(X).           (stratum 0, seed reach(a))
    // unreach(X) :- node(X), not reach(X).        (stratum 1)
    let prog = CoreProgram {
        predicates: vec![
            predn("node", 1, Semiring::Bool, 0),
            predn("edge", 2, Semiring::Bool, 0),
            predn("reach", 1, Semiring::Bool, 0),
            predn("unreach", 1, Semiring::Bool, 1),
        ],
        rules: vec![
            rulek(
                head("reach", vec![v(1)]),
                vec![pos("edge", vec![v(0), v(1)]), pos("reach", vec![v(0)])],
                2,
                0,
            ),
            rulek(
                head("unreach", vec![v(0)]),
                vec![pos("node", vec![v(0)]), neg("reach", vec![v(0)])],
                1,
                1,
            ),
        ],
        num_strata: 2,
    };

    let mut dict = SymbolDict::new();
    let mut sym = |s: &str| GroundVal::Sym(dict.intern(s));
    let edb = vec![
        ("node", vec![sym("a")], Ann::Unit),
        ("node", vec![sym("b")], Ann::Unit),
        ("node", vec![sym("c")], Ann::Unit),
        ("edge", vec![sym("a"), sym("b")], Ann::Unit),
        ("reach", vec![sym("a")], Ann::Unit),
    ];
    let db = agree(&prog, &edb);

    let unreach: Vec<GroundVal> = db
        .relation("unreach")
        .unwrap()
        .rows
        .keys()
        .map(|t| t[0])
        .collect();
    assert_eq!(
        unreach,
        vec![GroundVal::Sym(dict.get("c").unwrap())],
        "only c is unreachable"
    );
    // sanity: reach saturated to {a, b}
    assert_eq!(db.relation("reach").unwrap().len(), 2);
}

#[test]
fn negation_against_same_stratum_is_rejected() {
    // p(X) :- q(X), not p(X).  — p negated in its own stratum → loud error.
    let prog = CoreProgram {
        predicates: vec![
            predn("q", 1, Semiring::Bool, 0),
            predn("p", 1, Semiring::Bool, 0),
        ],
        rules: vec![rulek(
            head("p", vec![v(0)]),
            vec![pos("q", vec![v(0)]), neg("p", vec![v(0)])],
            1,
            0,
        )],
        num_strata: 1,
    };
    let mut dict = SymbolDict::new();
    let edb = vec![("q", vec![GroundVal::Sym(dict.intern("a"))], Ann::Unit)];
    let err = run(&prog, &edb).unwrap_err();
    assert!(
        matches!(err, EvalError::NegationNotStratified { .. }),
        "got {err:?}"
    );
    assert!(matches!(
        run_semi_naive(&prog, &edb).unwrap_err(),
        EvalError::NegationNotStratified { .. }
    ));
}

// --- EVAL-6: aggregates between strata ---------------------------------------

#[test]
fn count_aggregate_outdegree() {
    // outdeg(X, count⟨Y⟩) :- edge(X,Y).
    let prog = CoreProgram {
        predicates: vec![
            predn("edge", 2, Semiring::Bool, 0),
            predn("outdeg", 2, Semiring::Bool, 1),
        ],
        rules: vec![rulek(
            head("outdeg", vec![v(0), agg(AggOp::Count, 1)]),
            vec![pos("edge", vec![v(0), v(1)])],
            2,
            1,
        )],
        num_strata: 2,
    };
    let mut dict = SymbolDict::new();
    let mut sym = |s: &str| GroundVal::Sym(dict.intern(s));
    let edb = vec![
        ("edge", vec![sym("a"), sym("b")], Ann::Unit),
        ("edge", vec![sym("a"), sym("c")], Ann::Unit),
        ("edge", vec![sym("b"), sym("c")], Ann::Unit),
    ];
    let db = agree(&prog, &edb);
    let deg = |x: &str| -> Option<i64> {
        db.relation("outdeg")
            .unwrap()
            .rows
            .keys()
            .find(|t| t[0] == GroundVal::Sym(dict.get(x).unwrap()))
            .map(|t| match t[1] {
                GroundVal::Int(n) => n,
                _ => panic!("count column must be Int"),
            })
    };
    assert_eq!(deg("a"), Some(2));
    assert_eq!(deg("b"), Some(1));
}

#[test]
fn sum_aggregate() {
    // total(X, sum⟨V⟩) :- w(X,V).
    let prog = CoreProgram {
        predicates: vec![
            predn("w", 2, Semiring::Bool, 0),
            predn("total", 2, Semiring::Bool, 1),
        ],
        rules: vec![rulek(
            head("total", vec![v(0), agg(AggOp::Sum, 1)]),
            vec![pos("w", vec![v(0), v(1)])],
            2,
            1,
        )],
        num_strata: 2,
    };
    let mut dict = SymbolDict::new();
    let mut sym = |s: &str| GroundVal::Sym(dict.intern(s));
    let edb = vec![
        ("w", vec![sym("a"), GroundVal::Int(3)], Ann::Unit),
        ("w", vec![sym("a"), GroundVal::Int(4)], Ann::Unit),
        ("w", vec![sym("b"), GroundVal::Int(5)], Ann::Unit),
    ];
    let db = agree(&prog, &edb);
    let total = |x: &str| -> Option<i64> {
        db.relation("total")
            .unwrap()
            .rows
            .keys()
            .find(|t| t[0] == GroundVal::Sym(dict.get(x).unwrap()))
            .map(|t| match t[1] {
                GroundVal::Int(n) => n,
                _ => panic!(),
            })
    };
    assert_eq!(total("a"), Some(7));
    assert_eq!(total("b"), Some(5));
}

// --- EVAL-7: negative-weight cycle detection ---------------------------------

#[test]
fn negative_weight_cycle_is_detected() {
    // reach(X,Y) :- edge(X,Y).  reach(X,Z) :- edge(X,Y), reach(Y,Z).
    // edge a->b (1), b->a (-2): the cycle a→b→a has weight -1 → diverges.
    let prog = CoreProgram {
        predicates: vec![
            predn("edge", 2, Semiring::Trop, 0),
            predn("reach", 2, Semiring::Trop, 0),
        ],
        rules: vec![
            rulek(
                head("reach", vec![v(0), v(1)]),
                vec![pos("edge", vec![v(0), v(1)])],
                2,
                0,
            ),
            rulek(
                head("reach", vec![v(0), v(2)]),
                vec![
                    pos("edge", vec![v(0), v(1)]),
                    pos("reach", vec![v(1), v(2)]),
                ],
                3,
                0,
            ),
        ],
        num_strata: 1,
    };
    let mut dict = SymbolDict::new();
    let mut sym = |s: &str| GroundVal::Sym(dict.intern(s));
    let edb = vec![
        ("edge", vec![sym("a"), sym("b")], Ann::W(Weight::Finite(1))),
        ("edge", vec![sym("b"), sym("a")], Ann::W(Weight::Finite(-2))),
    ];
    let err = run(&prog, &edb).unwrap_err();
    assert!(matches!(err, EvalError::NegativeWeightCycle), "got {err:?}");
    assert!(matches!(
        run_semi_naive(&prog, &edb).unwrap_err(),
        EvalError::NegativeWeightCycle
    ));
}
