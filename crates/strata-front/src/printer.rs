//! Canonical pretty-printer: High-IR → surface. [FRONT-8, D2, D12]
//!
//! The inverse direction of the D2 bijection on canonical form:
//! `parse(print(ir)) == ir`. Deterministic layout (one item per line, fixed
//! spacing, default effects omitted) so formatting is idempotent and the
//! parse→print→parse roundtrip is a free grammar test (D12). Covers the whole
//! grammar, including the parse-but-unsupported constructs (D5).

use std::fmt::Write;

use strata_ir::high::program::{AggOp, Atom, ItemKind, Literal, Program, QueryKind, Term};
use strata_ir::high::sig::{
    Annotation, ArgType, Completeness, Determinism, Effects, Signature, Termination,
};
use strata_ir::trop::Weight;

/// Render a whole program to canonical surface (trailing newline).
pub fn print_program(p: &Program) -> String {
    let mut out = String::new();
    for (i, item) in p.items.iter().enumerate() {
        // A preceding blank line (trivia), except before the very first item.
        if item.trivia.blank_before && i > 0 {
            out.push('\n');
        }
        // Leading comments (trivia) are emitted verbatim, one per line (FRONT-10).
        for comment in &item.trivia.leading {
            out.push_str(comment);
            out.push('\n');
        }
        print_item(&mut out, &item.node);
        out.push('\n');
    }
    out
}

fn print_item(out: &mut String, node: &ItemKind) {
    match node {
        ItemKind::Domain(d) => {
            let _ = write!(out, "domain {}.", d.name);
        }
        ItemKind::Predicate(p) if p.neural.is_some() => {
            let model = &p.neural.as_ref().unwrap().model;
            let _ = write!(out, "neural {}(", p.name);
            for (i, a) in p.sig.args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&arg_type(a));
            }
            let _ = write!(out, ") from model {model:?}.");
        }
        ItemKind::Predicate(p) => {
            let _ = write!(out, "pred {}(", p.name);
            for (i, a) in p.sig.args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&arg_type(a));
            }
            let _ = write!(out, "): {}", annotation(&p.sig.annotation));
            out.push_str(&effects(&p.sig));
            out.push('.');
        }
        ItemKind::Rule(r) => {
            print_atom(out, &r.head);
            out.push_str(" :- ");
            for (i, lit) in r.body.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                match lit {
                    Literal::Pos(a) => print_atom(out, a),
                    Literal::Neg(a) => {
                        out.push_str("not ");
                        print_atom(out, a);
                    }
                }
            }
            out.push('.');
        }
        ItemKind::Fact(f) => {
            if let Some(w) = f.weight {
                let _ = write!(out, "{} :: ", weight(w));
            } else if let Some(pr) = f.prob {
                let _ = write!(out, "{pr:?} :: ");
            }
            print_atom(out, &f.atom);
            out.push('.');
        }
        ItemKind::Input(i) => {
            let _ = write!(out, "input {} from {:?}.", i.pred, i.path);
        }
        ItemKind::Query(q) => {
            out.push_str(match q.kind {
                QueryKind::Plain => "? ",
                QueryKind::Prob => "?prob ",
                QueryKind::Grad => "?grad ",
            });
            print_atom(out, &q.atom);
            out.push('.');
        }
        ItemKind::Pragma(p) => out.push_str(match p {
            strata_ir::high::program::Pragma::Terms => "@terms.",
            strata_ir::high::program::Pragma::Asp => "@asp.",
        }),
    }
}

fn print_atom(out: &mut String, a: &Atom) {
    let _ = write!(out, "{}(", a.pred);
    for (i, t) in a.args.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&term(t));
    }
    out.push(')');
}

fn term(t: &Term) -> String {
    match t {
        Term::Var { name } => name.clone(),
        Term::Const { name } => name.clone(),
        Term::Int { value } => value.to_string(),
        Term::Agg { op, var } => format!("{}<{}>", agg_op(*op), var),
        Term::Compound { functor, args } => {
            let inner: Vec<String> = args.iter().map(term).collect();
            format!("{functor}({})", inner.join(", "))
        }
    }
}

fn arg_type(a: &ArgType) -> String {
    match a {
        ArgType::Domain { name } => name.clone(),
        ArgType::Int => "int".to_string(),
        ArgType::Term { name } => format!("term {name}"),
    }
}

fn annotation(a: &Annotation) -> String {
    match a {
        Annotation::Bool => "Bool".into(),
        Annotation::Trop => "Trop".into(),
        Annotation::Prov => "Prov".into(),
        // Always explicit, so the k a program runs with is the k it shows.
        Annotation::ProvK { k } => format!("Prov_k({k})"),
    }
}

/// Only non-default effects are printed (so the roundtrip is a fixpoint).
fn effects(sig: &Signature) -> String {
    let e: &Effects = &sig.effects;
    let mut s = String::new();
    if e.termination == Termination::Partial {
        s.push_str(" partial");
    }
    if e.completeness == Completeness::SoundOnly {
        s.push_str(" sound_only");
    }
    if e.determinism == Determinism::Stochastic {
        s.push_str(" stochastic");
    }
    s
}

fn weight(w: Weight) -> String {
    match w {
        Weight::Finite(n) => n.to_string(),
        Weight::PosInf => "inf".to_string(),
    }
}

fn agg_op(op: AggOp) -> &'static str {
    match op {
        AggOp::Min => "min",
        AggOp::Max => "max",
        AggOp::Sum => "sum",
        AggOp::Count => "count",
        AggOp::ProbOr => "prob_or",
    }
}
