//! Slice 1 demo + drift guard. [IR-4/IR-6/IR-7]
//!
//! - Hand-authored transitive-closure (Bool) and SSSP (Trop, +∞) High-IR
//!   round-trip byte-stably and load through the version gate.
//! - A hand-written single-stratum Core-IR round-trips and exposes strata order.
//! - The committed JSON Schema matches the generated one (INFRA-9 preview).

use strata_ir::core::{CoreAtom, CoreLiteral, CorePred, CoreProgram, CoreRule, CoreTerm, Semiring};
use strata_ir::high::program::{atom, cst, var, AggOp};
use strata_ir::high::program::{DomainDecl, PredDecl};
use strata_ir::high::sig::{Annotation, ArgType, Effects, Signature};
use strata_ir::high::{Fact, Item, ItemKind, Literal, Program, Rule};
use strata_ir::schema::load_program;
use strata_ir::trop::Weight;

fn bool_pred(name: &str) -> Item {
    Item::new(ItemKind::Predicate(PredDecl {
        name: name.into(),
        sig: Signature {
            args: vec![
                ArgType::Domain {
                    name: "node".into(),
                },
                ArgType::Domain {
                    name: "node".into(),
                },
            ],
            annotation: Annotation::Bool,
            effects: Effects::default(),
        },
        neural: None,
    }))
}

/// path(X,Y) :- edge(X,Y).  path(X,Z) :- edge(X,Y), path(Y,Z).  + a few edges.
fn transitive_closure() -> Program {
    Program::new(vec![
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
    ])
}

/// A tropical program exercising both a finite weight and +∞ in a Fact.
fn sssp() -> Program {
    let trop_pred = |name: &str| {
        Item::new(ItemKind::Predicate(PredDecl {
            name: name.into(),
            sig: Signature {
                args: vec![
                    ArgType::Domain {
                        name: "node".into(),
                    },
                    ArgType::Domain {
                        name: "node".into(),
                    },
                ],
                annotation: Annotation::Trop,
                effects: Effects::default(),
            },
            neural: None,
        }))
    };
    Program::new(vec![
        Item::new(ItemKind::Domain(DomainDecl {
            name: "node".into(),
        })),
        trop_pred("edge"),
        // reach(X,Z) min-plus over edges
        Item::new(ItemKind::Rule(Rule {
            head: atom("reach", vec![var("X"), var("Z")]),
            body: vec![
                Literal::Pos(atom("edge", vec![var("X"), var("Y")])),
                Literal::Pos(atom("reach", vec![var("Y"), var("Z")])),
            ],
        })),
        Item::new(ItemKind::Fact(Fact {
            atom: atom("edge", vec![cst("a"), cst("b")]),
            weight: Some(Weight::Finite(5)),
            prob: None,
        })),
        Item::new(ItemKind::Fact(Fact {
            atom: atom("edge", vec![cst("b"), cst("c")]),
            weight: Some(Weight::PosInf),
            prob: None,
        })),
    ])
}

fn assert_byte_stable_roundtrip(p: &Program) {
    let json1 = serde_json::to_string_pretty(p).expect("serialize");
    let back: Program = serde_json::from_str(&json1).expect("deserialize");
    assert_eq!(p, &back, "value roundtrip");
    let json2 = serde_json::to_string_pretty(&back).expect("reserialize");
    assert_eq!(json1, json2, "byte-stable roundtrip");
    // loads through the version gate (D11)
    load_program(&json1).expect("version-gated load");
}

#[test]
fn transitive_closure_roundtrips() {
    assert_byte_stable_roundtrip(&transitive_closure());
}

#[test]
fn sssp_with_inf_roundtrips() {
    let p = sssp();
    assert_byte_stable_roundtrip(&p);
    // +∞ renders as the string "inf" inside the fact.
    let json = serde_json::to_string(&p).unwrap();
    assert!(
        json.contains(r#""weight":"inf""#),
        "PosInf must render as \"inf\": {json}"
    );
    assert!(
        json.contains(r#""weight":5"#),
        "finite weight must render as bare int"
    );
}

#[test]
fn adjacent_tag_shape_matches_docs() {
    // The item/term encoding must match docs/ir-encoding.md (kind/data).
    let p = transitive_closure();
    let v: serde_json::Value = serde_json::to_value(&p).unwrap();
    let first = &v["items"][0];
    assert_eq!(first["kind"], "domain");
    assert_eq!(first["data"]["name"], "node");
}

#[test]
fn core_ir_tc_roundtrips_and_exposes_strata() {
    // path(X,Z) :- edge(X,Y), path(Y,Z).  Vars: X=0, Y=1, Z=2.
    let core = CoreProgram {
        predicates: vec![
            CorePred {
                name: "edge".into(),
                arity: 2,
                semiring: Semiring::Bool,
                stratum: 0,
            },
            CorePred {
                name: "path".into(),
                arity: 2,
                semiring: Semiring::Bool,
                stratum: 0,
            },
        ],
        rules: vec![CoreRule {
            head: CoreAtom {
                pred: "path".into(),
                args: vec![CoreTerm::Var { slot: 0 }, CoreTerm::Var { slot: 2 }],
            },
            body: vec![
                CoreLiteral::Pos(CoreAtom {
                    pred: "edge".into(),
                    args: vec![CoreTerm::Var { slot: 0 }, CoreTerm::Var { slot: 1 }],
                }),
                CoreLiteral::Pos(CoreAtom {
                    pred: "path".into(),
                    args: vec![CoreTerm::Var { slot: 1 }, CoreTerm::Var { slot: 2 }],
                }),
            ],
            stratum: 0,
            var_count: 3,
            neg_weight_cycle_check: false,
        }],
        num_strata: 1,
    };
    let json = serde_json::to_string_pretty(&core).unwrap();
    let back: CoreProgram = serde_json::from_str(&json).unwrap();
    assert_eq!(core, back);
    assert_eq!(back.num_strata, 1);
    assert_eq!(back.rules_in_stratum(0).count(), 1);
    assert_eq!(back.predicates[1].semiring, Semiring::Bool);
    // sanity: an aggregate core term also roundtrips
    let agg = CoreTerm::Agg {
        op: AggOp::Min,
        slot: 1,
    };
    assert_eq!(
        serde_json::from_str::<CoreTerm>(&serde_json::to_string(&agg).unwrap()).unwrap(),
        agg
    );
}

#[test]
fn published_schema_matches_generated() {
    let generated = {
        let mut s = strata_ir::schema::high_ir_schema_json();
        s.push('\n');
        s
    };
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../schema/high-ir.schema.json"
    );
    let committed = std::fs::read_to_string(path).expect(
        "schema/high-ir.schema.json missing — run `cargo run -p strata-ir --example gen_schema`",
    );
    assert_eq!(
        committed, generated,
        "published schema is stale — regenerate with `cargo run -p strata-ir --example gen_schema`"
    );
}
