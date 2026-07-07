//! `strata` — the single user-facing binary. [CLI-1/2/3/4/6/7/8, D10]
//!
//! Subcommands: `check | run | fmt | ir`, with a global `--error-format=text|json`
//! (D9). Ties strata-front + strata-check + strata-eval into the end-to-end path
//! text → parse → check → Core-IR → interpret → result.

use std::path::Path;
use std::process::ExitCode as ProcExit;

use strata_check::check_program;
use strata_eval::{run_semi_naive, run_terms, Ann, GroundVal};
use strata_front::{format, parse, print_program};
use strata_ir::core::CoreProgram;
use strata_ir::dict::SymbolDict;
use strata_ir::high::program::QueryKind;
use strata_ir::terms::TermTable;
use strata_ir::value::GroundFact;

mod asp;
mod prob;
mod prov;
mod render;

use asp::{is_asp, run_asp};
use prob::{grad_mode, prob_mode, run_grad, run_prob};
use prov::{prov_modes, run_prov_display};
use render::render_db;
use strata_k::load_inputs;

/// Process exit codes. Defined once, reused by every subcommand. [CLI-1]
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum Code {
    Ok = 0,
    /// Any error-severity diagnostic was reported.
    Diagnostics = 1,
    /// Bad CLI usage.
    Usage = 2,
    /// Runtime fault (e.g. Trop i64 overflow, D6).
    Runtime = 4,
}

fn main() -> ProcExit {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = dispatch(&args);
    ProcExit::from(code as u8)
}

fn dispatch(args: &[String]) -> Code {
    match args.first().map(String::as_str) {
        Some("check") => cmd_check(&args[1..]),
        Some("run") => cmd_run(&args[1..]),
        Some("fmt") => cmd_fmt(&args[1..]),
        Some("ir") => cmd_ir(&args[1..]),
        Some("--help") | Some("-h") | None => {
            print_help();
            Code::Ok
        }
        Some(other) => {
            eprintln!("strata: unknown subcommand `{other}`\n");
            print_help();
            Code::Usage
        }
    }
}

fn print_help() {
    eprintln!(
        "strata {} — Strata/K (Phase 0)\n\n\
         USAGE:\n  \
           strata check <file.strata> [--error-format=text|json]\n  \
           strata run   <file.strata> [--semi-naive] [--error-format=text|json]\n  \
           strata fmt   <file.strata> [--check]\n  \
           strata ir    <file> --to json|surface\n",
        strata_ir::IR_VERSION_STR
    );
}

// --- argument helpers --------------------------------------------------------

fn positional(args: &[String]) -> Option<&str> {
    args.iter()
        .find(|a| !a.starts_with('-'))
        .map(String::as_str)
}
fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}
fn opt_value<'a>(args: &'a [String], key: &str) -> Option<&'a str> {
    args.iter().find_map(|a| a.strip_prefix(key))
}
fn wants_json(args: &[String]) -> bool {
    opt_value(args, "--error-format=") == Some("json")
}

fn read(args: &[String]) -> Result<(String, String), Code> {
    let Some(path) = positional(args) else {
        eprintln!("strata: expected a file argument");
        return Err(Code::Usage);
    };
    match std::fs::read_to_string(path) {
        Ok(src) => Ok((path.to_string(), src)),
        Err(e) => {
            eprintln!("strata: cannot read {path}: {e}");
            Err(Code::Usage)
        }
    }
}

/// Render diagnostics either as text (with source context) or JSON.
fn emit(diags: &strata_ir::diag::Diagnostics, src: &str, json: bool) {
    if json {
        println!("{}", diags.render_json());
    } else {
        eprint!("{}", diags.render_text(src));
    }
}

// --- subcommands -------------------------------------------------------------

fn cmd_check(args: &[String]) -> Code {
    let (_, src) = match read(args) {
        Ok(v) => v,
        Err(c) => return c,
    };
    let json = wants_json(args);
    let (prog, pdiags) = parse(&src);
    if pdiags.has_errors() {
        emit(&pdiags, &src, json);
        return Code::Diagnostics;
    }
    if is_asp(&prog) {
        // @asp uses stable-model semantics (unstratified negation allowed), so
        // it skips stratification — but not the mandatory-signature promise:
        // declarations and arity are checked here too.
        return match strata_check::check_asp_declarations(&prog) {
            Ok(()) => {
                if json {
                    println!("[]");
                } else {
                    eprintln!("ok (asp)");
                }
                Code::Ok
            }
            Err(cdiags) => {
                emit(&cdiags, &src, json);
                Code::Diagnostics
            }
        };
    }
    match check_program(&prog) {
        Ok(_) => {
            if !json {
                eprintln!("ok");
            } else {
                println!("[]");
            }
            Code::Ok
        }
        Err(cdiags) => {
            emit(&cdiags, &src, json);
            Code::Diagnostics
        }
    }
}

fn cmd_run(args: &[String]) -> Code {
    let (path, src) = match read(args) {
        Ok(v) => v,
        Err(c) => return c,
    };
    let json = wants_json(args);
    let (prog, pdiags) = parse(&src);
    if pdiags.has_errors() {
        emit(&pdiags, &src, json);
        return Code::Diagnostics;
    }
    if is_asp(&prog) {
        // ASP (Phase 5): declarations first (the signature promise is global),
        // then stable models via the reference solver.
        if let Err(cdiags) = strata_check::check_asp_declarations(&prog) {
            emit(&cdiags, &src, json);
            return Code::Diagnostics;
        }
        return match run_asp(&prog) {
            Ok(out) => {
                print!("{out}");
                Code::Ok
            }
            Err(e) => {
                eprintln!("strata: {e}");
                Code::Runtime
            }
        };
    }
    let mut checked = match check_program(&prog) {
        Ok(c) => c,
        Err(cdiags) => {
            emit(&cdiags, &src, json);
            return Code::Diagnostics;
        }
    };

    // Load `input pred from "file.tsv"` EDB (CLI-5); paths resolve relative to
    // the source file. Interns into the SAME dictionary check produced.
    let base = Path::new(&path).parent().unwrap_or_else(|| Path::new("."));
    // The facade loader pushes certain rows into checked.edb and neural rows
    // (trailing probability) into checked.prob_edb — the path library users get.
    if let Err(e) = load_inputs(&prog, &mut checked, base) {
        eprintln!("strata: {e}");
        return Code::Usage;
    }
    let facts = checked.edb.clone();

    // режим B: a `?grad` query differentiates the marginal; a `?prob` query (or
    // any probabilistic fact) asks for the marginal itself (Phase 4). Prov/Prov_k
    // predicates route through provenance capture + circuit compilation; with no
    // query, a plain run of a Prov program prints each fact's pedigree.
    // Otherwise a plain evaluation.
    let has_prob_queries = checked.queries.iter().any(|q| q.kind == QueryKind::Prob);
    let result = if grad_mode(&checked) || has_prob_queries {
        // Answer ?prob queries, then ?grad queries (a program may carry both).
        (|| {
            let mut combined = String::new();
            if has_prob_queries {
                combined.push_str(&run_prob(&mut checked, &facts)?);
            }
            if grad_mode(&checked) {
                combined.push_str(&run_grad(&mut checked, &facts)?);
            }
            Ok(append_terms_status(combined, &checked.terms))
        })()
    } else if !prov_modes(&checked).is_empty() {
        run_prov_display(&mut checked, &facts).map(|out| append_terms_status(out, &checked.terms))
    } else if prob_mode(&checked) {
        run_prob(&mut checked, &facts).map(|out| append_terms_status(out, &checked.terms))
    } else {
        let semi = has_flag(args, "--semi-naive");
        run_program(
            &checked.core,
            &checked.dict,
            &facts,
            semi,
            &mut checked.terms,
        )
    };
    match result {
        Ok(out) => {
            print!("{out}");
            Code::Ok
        }
        Err(e) => {
            eprintln!("strata: runtime error: {e}");
            Code::Runtime
        }
    }
}

fn cmd_fmt(args: &[String]) -> Code {
    let (_, src) = match read(args) {
        Ok(v) => v,
        Err(c) => return c,
    };
    match format(&src) {
        Ok(canon) => {
            if has_flag(args, "--check") {
                if canon == src {
                    Code::Ok
                } else {
                    eprintln!("strata: file is not formatted");
                    Code::Diagnostics
                }
            } else {
                print!("{canon}");
                Code::Ok
            }
        }
        Err(diags) => {
            emit(&diags, &src, false);
            Code::Diagnostics
        }
    }
}

fn cmd_ir(args: &[String]) -> Code {
    let (_, src) = match read(args) {
        Ok(v) => v,
        Err(c) => return c,
    };
    match opt_value(args, "--to=").or_else(|| {
        // also accept `--to json`
        args.iter()
            .position(|a| a == "--to")
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    }) {
        Some("json") => {
            let (prog, diags) = parse(&src);
            if diags.has_errors() {
                emit(&diags, &src, false);
                return Code::Diagnostics;
            }
            println!("{}", serde_json::to_string_pretty(&prog).unwrap());
            Code::Ok
        }
        Some("surface") => match serde_json::from_str::<strata_ir::high::Program>(&src) {
            Ok(prog) => {
                print!("{}", print_program(&prog));
                Code::Ok
            }
            Err(e) => {
                eprintln!("strata: invalid High-IR JSON: {e}");
                Code::Usage
            }
        },
        _ => {
            eprintln!("strata: `ir` needs --to json|surface");
            Code::Usage
        }
    }
}

// --- evaluation + output -----------------------------------------------------

/// If the term table dropped derivations at its depth bound, the answers are a
/// sound under-approximation — the режим-B outputs must say so, exactly like a
/// plain run does (spec §1.4 `Sound[T]`).
fn append_terms_status(mut out: String, terms: &TermTable) -> String {
    if !terms.is_complete() {
        out.push_str(&format!(
            "% status: Sound (possibly incomplete) — {} derivation(s) dropped at depth bound {}\n",
            terms.dropped(),
            strata_ir::terms::DEFAULT_MAX_DEPTH
        ));
    }
    out
}

/// Evaluate a Core-IR program over `facts`, rendering relations to canonical text.
fn run_program(
    core: &CoreProgram,
    dict: &SymbolDict,
    facts: &[GroundFact],
    semi_naive: bool,
    terms: &mut TermTable,
) -> Result<String, String> {
    let edb: Vec<(&str, Vec<GroundVal>, Ann)> = facts
        .iter()
        .map(|f| (f.pred.as_str(), f.args.clone(), Ann::from_weight(f.weight)))
        .collect();
    // `terms` already holds the compound EDB facts (interned by the checker) and
    // outlives the database so constructed terms can be rendered; the depth bound
    // guarantees termination for `@terms` programs.
    // Semi-naive carries no depth bound: a term-constructing program would
    // diverge under it, so `@terms` programs always take the naive engine
    // (the bounded, sound-but-incomplete one), flag or no flag.
    let program_uses_terms = !terms.is_empty()
        || core.rules.iter().any(|r| {
            use strata_ir::core::{CoreLiteral, CoreTerm};
            let compound = |t: &CoreTerm| matches!(t, CoreTerm::Compound { .. });
            r.head.args.iter().any(compound)
                || r.body.iter().any(|l| {
                    let (CoreLiteral::Pos(a) | CoreLiteral::Neg(a)) = l;
                    a.args.iter().any(compound)
                })
        });
    let db = if semi_naive && !program_uses_terms {
        run_semi_naive(core, &edb).map_err(|e| e.to_string())?
    } else {
        run_terms(core, &edb, terms).map_err(|e| e.to_string())?
    };
    let mut out = render_db(&db, dict, terms);
    if !terms.is_complete() {
        out.push_str(&format!(
            "% status: Sound (possibly incomplete) — {} derivation(s) dropped at depth bound {}\n",
            terms.dropped(),
            strata_ir::terms::DEFAULT_MAX_DEPTH
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_check::Checked;

    fn eval_src(src: &str, semi: bool) -> String {
        let (prog, diags) = parse(src);
        assert!(!diags.has_errors(), "{}", diags.render_text(src));
        let mut checked = check_program(&prog).expect("check");
        let edb = checked.edb.clone();
        run_program(&checked.core, &checked.dict, &edb, semi, &mut checked.terms).expect("eval")
    }

    const TC: &str = "\
pred edge(node, node): Bool.
pred path(node, node): Bool.
path(X, Y) :- edge(X, Y).
path(X, Z) :- edge(X, Y), path(Y, Z).
edge(a, b).
edge(b, c).
";

    #[test]
    fn end_to_end_transitive_closure() {
        let out = eval_src(TC, false);
        // a→b→c gives path {a-b, b-c, a-c} plus the edge facts.
        assert!(out.contains("path(a, b)"), "{out}");
        assert!(out.contains("path(b, c)"), "{out}");
        assert!(out.contains("path(a, c)"), "{out}");
        // naive and semi-naive agree end-to-end
        assert_eq!(out, eval_src(TC, true));
    }

    #[test]
    fn end_to_end_sssp_trop() {
        let src = "\
pred edge(node, node): Trop.
pred reach(node, node): Trop.
reach(X, Y) :- edge(X, Y).
reach(X, Z) :- edge(X, Y), reach(Y, Z).
2 :: edge(a, b).
3 :: edge(b, c).
10 :: edge(a, c).
";
        let out = eval_src(src, false);
        // shortest a→c is min(10, 2+3) = 5
        assert!(out.contains("reach(a, c) = 5"), "{out}");
    }

    #[test]
    fn end_to_end_probabilistic_query() {
        // Two routes a→c (direct 0.5, and via b at 0.5·0.5) ⇒ P = 0.625.
        let src = "\
pred edge(node, node): Bool.
pred path(node, node): Bool.
path(X, Y) :- edge(X, Y).
path(X, Z) :- edge(X, Y), path(Y, Z).
0.5 :: edge(a, c).
0.5 :: edge(a, b).
0.5 :: edge(b, c).
?prob path(a, c).
";
        let (prog, diags) = parse(src);
        assert!(!diags.has_errors(), "{}", diags.render_text(src));
        let mut checked = check_program(&prog).expect("check");
        assert!(prob_mode(&checked));
        let edb = checked.edb.clone();
        let out = run_prob(&mut checked, &edb).expect("prob run");
        assert_eq!(out, "0.625 :: path(a, c)\n", "{out}");
    }

    fn checked_of(src: &str) -> Checked {
        let (prog, diags) = parse(src);
        assert!(!diags.has_errors(), "{}", diags.render_text(src));
        check_program(&prog).expect("check")
    }

    #[test]
    fn end_to_end_prov_pedigree_display() {
        // The ch11-prov shape: a plain run prints each Prov fact's marginal and
        // its pedigree — one ⇐ line per minimal proof.
        let src = "\
domain firm.
pred owns(firm, firm): Bool.
pred controls(firm, firm): Prov.
0.9 :: owns(acme, shell).
0.8 :: owns(shell, target).
0.3 :: owns(acme, target).
controls(X, Y) :- owns(X, Y).
controls(X, Z) :- owns(X, Y), owns(Y, Z).
";
        let mut checked = checked_of(src);
        let edb = checked.edb.clone();
        let out = run_prov_display(&mut checked, &edb).expect("prov display");
        // P = 0.9·0.8 + 0.3 − 0.9·0.8·0.3 = 0.804, both proofs listed.
        assert!(out.contains("0.804 :: controls(acme, target)"), "{out}");
        assert!(
            out.contains("  ⇐ [0.9 :: owns(acme, shell)] ∧ [0.8 :: owns(shell, target)]"),
            "{out}"
        );
        assert!(out.contains("  ⇐ [0.3 :: owns(acme, target)]"), "{out}");
        // Base soft facts print as marginals, without pedigree lines.
        assert!(out.contains("0.9 :: owns(acme, shell)\n"), "{out}");
    }

    #[test]
    fn end_to_end_prob_on_prov_matches_enumeration() {
        // The same query answered by the circuit (Prov pred) and the exact
        // enumeration oracle (Bool pred) must agree: 0.625 both ways.
        let bool_src = "\
pred edge(node, node): Bool.
pred path(node, node): Bool.
path(X, Y) :- edge(X, Y).
path(X, Z) :- edge(X, Y), path(Y, Z).
0.5 :: edge(a, c).
0.5 :: edge(a, b).
0.5 :: edge(b, c).
?prob path(a, c).
";
        let prov_src = "\
pred edge(node, node): Bool.
pred path(node, node): Bool.
pred answer(node, node): Prov.
path(X, Y) :- edge(X, Y).
path(X, Z) :- edge(X, Y), path(Y, Z).
answer(X, Y) :- path(X, Y).
0.5 :: edge(a, c).
0.5 :: edge(a, b).
0.5 :: edge(b, c).
?prob answer(a, c).
";
        let mut b = checked_of(bool_src);
        let b_edb = b.edb.clone();
        let enumerated = run_prob(&mut b, &b_edb).unwrap();
        let mut pv = checked_of(prov_src);
        let pv_edb = pv.edb.clone();
        let circuit = run_prob(&mut pv, &pv_edb).unwrap();
        assert_eq!(enumerated, "0.625 :: path(a, c)\n");
        assert_eq!(circuit, "0.625 :: answer(a, c)\n");
    }

    #[test]
    fn end_to_end_provk_lower_bound_bites_and_says_so() {
        // Two routes a→c (0.5 direct, 0.25 via b); Prov_k(1) keeps only the
        // best proof → 0.5, printed as a declared lower bound (И4).
        let src = "\
pred edge(node, node): Bool.
pred reach(node, node): Prov_k(1).
reach(X, Y) :- edge(X, Y).
reach(X, Z) :- edge(X, Y), reach(Y, Z).
0.5 :: edge(a, c).
0.5 :: edge(a, b).
0.5 :: edge(b, c).
?prob reach(a, c).
";
        let mut checked = checked_of(src);
        let edb = checked.edb.clone();
        let out = run_prob(&mut checked, &edb).expect("prob run");
        assert_eq!(out, "0.5 :: reach(a, c)  (lower bound, top-1)\n", "{out}");
    }

    #[test]
    fn end_to_end_grad_on_prov_circuit_keeps_model_labels() {
        // ?grad on a Prov predicate differentiates the compiled circuit; a
        // neural leaf still names the model the gradient backpropagates into.
        let src = "\
domain firm.
neural flag(firm) from model \"aml_gnn\".
pred investigate(firm): Prov.
investigate(X) :- flag(X).
0.9 :: flag(acme).
?grad investigate(acme).
";
        let mut checked = checked_of(src);
        let edb = checked.edb.clone();
        let out = run_grad(&mut checked, &edb).expect("grad run");
        assert_eq!(
            out, "0.9 :: investigate(acme)\n  ∂/∂[0.9 :: flag(acme)] = 1  (→ model \"aml_gnn\")\n",
            "{out}"
        );
    }

    #[test]
    fn end_to_end_prov_negation_is_a_dual_literal() {
        // not over a soft EDB fact: P(ok) = 1 − 0.5, and the pedigree shows ¬.
        let src = "\
domain firm.
pred node(firm): Bool.
pred flag(firm): Bool.
pred ok(firm): Prov.
node(a).
0.5 :: flag(a).
ok(X) :- node(X), not flag(X).
";
        let mut checked = checked_of(src);
        let edb = checked.edb.clone();
        let out = run_prov_display(&mut checked, &edb).expect("prov display");
        assert!(out.contains("0.5 :: ok(a)"), "{out}");
        assert!(out.contains("  ⇐ ¬[0.5 :: flag(a)]"), "{out}");
    }

    #[test]
    fn end_to_end_prov_beyond_the_enumeration_limit() {
        // 25 soft facts: 2^25 worlds is past the enumeration cap, but the Prov
        // circuit answers exactly (the capability the annotation buys).
        let mut src = String::from(
            "pred edge(node, node): Bool.\n\
             pred path(node, node): Bool.\n\
             pred answer(node, node): Prov.\n\
             path(X, Y) :- edge(X, Y).\n\
             path(X, Z) :- edge(X, Y), path(Y, Z).\n\
             answer(X, Y) :- path(X, Y).\n",
        );
        for i in 0..25 {
            src.push_str(&format!("0.9 :: edge(n{i}, n{}).\n", i + 1));
        }
        src.push_str("?prob answer(n0, n25).\n");
        let mut checked = checked_of(&src);
        let edb = checked.edb.clone();
        let out = run_prob(&mut checked, &edb).expect("circuit path runs");
        let p: f64 = out
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .expect("a marginal");
        assert!((p - 0.9f64.powi(25)).abs() < 1e-12, "{out}");
        // The Bool enumeration oracle refuses the same query at this size.
        let bool_src = src.replace(": Prov", ": Bool");
        let mut checked_bool = checked_of(&bool_src);
        let bool_edb = checked_bool.edb.clone();
        assert!(run_prob(&mut checked_bool, &bool_edb).is_err());
    }

    #[test]
    fn end_to_end_terms_construct_and_bound() {
        // @terms: build naturals via succ(X); the depth bound stops divergence
        // and marks the result sound-but-incomplete (spec §1.4).
        let src = "\
@terms.
domain elem.
pred nat(elem): Bool.
nat(zero).
nat(succ(X)) :- nat(X).
";
        let (prog, diags) = parse(src);
        assert!(!diags.has_errors(), "{}", diags.render_text(src));
        let mut checked = check_program(&prog).expect("check");
        let out = run_program(
            &checked.core,
            &checked.dict,
            &checked.edb.clone(),
            false,
            &mut checked.terms,
        )
        .expect("run");
        assert!(out.contains("nat(zero)"), "{out}");
        assert!(out.contains("nat(succ(zero))"), "{out}");
        assert!(out.contains("nat(succ(succ(zero)))"), "{out}");
        // divergence hit the bound → sound-but-incomplete status line.
        assert!(out.contains("Sound (possibly incomplete)"), "{out}");
    }

    #[test]
    fn semi_naive_flag_on_terms_program_terminates_bounded() {
        // Semi-naive has no depth bound; the runner must fall back to the
        // bounded naive engine instead of diverging (this used to hang).
        let src = "\
@terms.
domain elem.
pred nat(elem): Bool.
nat(zero).
nat(succ(X)) :- nat(X).
";
        let out = eval_src(src, true);
        assert!(out.contains("Sound (possibly incomplete)"), "{out}");
    }

    #[test]
    fn regime_b_reports_the_depth_bound_status() {
        // A ?prob answer over a depth-bounded @terms program is a sound
        // under-approximation and must say so, like a plain run does.
        let src = "\
@terms.
domain node.
pred q(node): Bool.
pred s(node): Bool.
0.5 :: q(box(a)).
s(X) :- q(box(X)).
s(succ(X)) :- s(X).
?prob s(a).
";
        let mut checked = checked_of(src);
        let edb = checked.edb.clone();
        let out = run_prob(&mut checked, &edb).expect("prob run");
        let out = append_terms_status(out, &checked.terms);
        assert!(out.contains("0.5 :: s(a)"), "{out}");
        assert!(out.contains("Sound (possibly incomplete)"), "{out}");
    }

    #[test]
    fn end_to_end_terms_decompose() {
        // @terms: construct box(X), then unify it back apart in a body — the
        // compound-term unification path.
        let src = "\
@terms.
domain elem.
pred base(elem): Bool.
pred boxed(elem): Bool.
pred unboxed(elem): Bool.
base(a).
base(b).
boxed(box(X)) :- base(X).
unboxed(Y) :- boxed(box(Y)).
";
        let (prog, diags) = parse(src);
        assert!(!diags.has_errors(), "{}", diags.render_text(src));
        let mut checked = check_program(&prog).expect("check");
        let out = run_program(
            &checked.core,
            &checked.dict,
            &checked.edb.clone(),
            false,
            &mut checked.terms,
        )
        .expect("run");
        assert!(out.contains("boxed(box(a))"), "{out}");
        assert!(
            out.contains("unboxed(a)") && out.contains("unboxed(b)"),
            "{out}"
        );
        // no divergence here → complete, no status line.
        assert!(!out.contains("incomplete"), "{out}");
    }

    #[test]
    fn end_to_end_gradient_query() {
        // Same two-route graph; ?grad path(a,c) returns the marginal and the
        // gradient w.r.t. each edge probability. With x=p(a,c), y=p(a,b),
        // z=p(b,c) all 0.5: P = x+yz-xyz = 0.625; ∂/∂x = 1-yz = 0.75;
        // ∂/∂y = z(1-x) = 0.25; ∂/∂z = y(1-x) = 0.25.
        let src = "\
pred edge(node, node): Bool.
pred path(node, node): Bool.
path(X, Y) :- edge(X, Y).
path(X, Z) :- edge(X, Y), path(Y, Z).
0.5 :: edge(a, c).
0.5 :: edge(a, b).
0.5 :: edge(b, c).
?grad path(a, c).
";
        let (prog, diags) = parse(src);
        assert!(!diags.has_errors(), "{}", diags.render_text(src));
        let mut checked = check_program(&prog).expect("check");
        assert!(grad_mode(&checked));
        let edb = checked.edb.clone();
        let out = run_grad(&mut checked, &edb).expect("grad run");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "0.625 :: path(a, c)", "{out}");
        // parse "  ∂/∂[…] = <value>" and check the three gradients.
        let vals: Vec<f64> = lines[1..4]
            .iter()
            .map(|l| l.rsplit_once('=').unwrap().1.trim().parse::<f64>().unwrap())
            .collect();
        let want = [0.75, 0.25, 0.25];
        for (got, w) in vals.iter().zip(&want) {
            assert!((got - w).abs() < 1e-9, "gradient {got} vs {w}\n{out}");
        }
    }

    #[test]
    fn end_to_end_neural_predicate() {
        // A neural predicate's atoms are the model's soft outputs; режим B + ?grad
        // run over them, and the gradient names the model it backprops into.
        let src = "\
domain firm.
domain label.
neural flag(firm, label) from model \"aml_gnn\".
pred alert(firm): Bool.
alert(F) :- flag(F, high).
0.8 :: flag(acme, high).
0.3 :: flag(acme, low).
?grad alert(acme).
";
        let (prog, diags) = parse(src);
        assert!(!diags.has_errors(), "{}", diags.render_text(src));
        let checked = check_program(&prog).expect("check");
        assert_eq!(
            checked.neural,
            vec![("flag".to_string(), "aml_gnn".to_string())]
        );
        let out = {
            let mut checked = checked;
            let edb = checked.edb.clone();
            run_grad(&mut checked, &edb).expect("neural grad run")
        };
        // P(alert(acme)) = P(flag(acme,high)) = 0.8; ∂/∂high = 1, ∂/∂low = 0.
        assert!(out.contains(":: alert(acme)"), "{out}");
        assert!(out.contains("flag(acme, high)] = 1"), "{out}");
        assert!(out.contains("flag(acme, low)] = 0"), "{out}");
        assert!(out.contains("(→ model \"aml_gnn\")"), "{out}");
    }

    #[test]
    fn neural_certain_fact_is_rejected() {
        // A plain (certain) fact on a neural predicate is an E1010 category error.
        let src = "\
domain firm.
domain label.
neural flag(firm, label) from model \"m\".
flag(acme, high).
";
        let (prog, diags) = parse(src);
        assert!(!diags.has_errors(), "parse: {}", diags.render_text(src));
        let err = check_program(&prog).expect_err("neural certain fact must fail");
        assert!(
            err.render_text(src).contains("E1010"),
            "{}",
            err.render_text(src)
        );
    }

    #[test]
    fn end_to_end_asp_even_cycle() {
        // @asp with unstratified negation: two stable models {a} and {b}.
        let src = "\
@asp.
pred a(): Bool.
pred b(): Bool.
a() :- not b().
b() :- not a().
";
        let (prog, diags) = parse(src);
        assert!(!diags.has_errors(), "{}", diags.render_text(src));
        assert!(is_asp(&prog));
        let out = run_asp(&prog).expect("asp run");
        assert_eq!(out, "Answer 1: {a}\nAnswer 2: {b}\n", "{out}");
    }

    fn write_temp(name: &str, content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("strata_inputs_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    fn run_with_inputs(src: &str, dir: &std::path::Path) -> String {
        let (prog, diags) = parse(src);
        assert!(!diags.has_errors(), "{}", diags.render_text(src));
        let mut checked = check_program(&prog).expect("check");
        load_inputs(&prog, &mut checked, dir).expect("inputs load");
        let facts = checked.edb.clone();
        if grad_mode(&checked) {
            run_grad(&mut checked, &facts).expect("grad")
        } else if prob_mode(&checked) {
            run_prob(&mut checked, &facts).expect("prob")
        } else {
            run_program(
                &checked.core,
                &checked.dict,
                &facts,
                false,
                &mut checked.terms,
            )
            .expect("eval")
        }
    }

    #[test]
    fn csv_edb_loading_with_quotes() {
        let p = write_temp("edges.csv", "a,b\n\"c,c\",d\n\"say \"\"hi\"\"\",e\n");
        let dir = p.parent().unwrap().to_path_buf();
        let out = run_with_inputs(
            "pred edge(node, node): Bool.\n\
             pred ok(node): Bool.\n\
             ok(X) :- edge(X, _Y).\n\
             input edge from \"edges.csv\".\n",
            &dir,
        );
        assert!(out.contains("edge(a, b)"), "{out}");
        assert!(out.contains("edge(c,c, d)"), "quoted comma survives: {out}");
        assert!(out.contains("edge(say \"hi\", e)"), "escaped quotes: {out}");
    }

    #[test]
    fn json_edb_loading_including_trop_weights() {
        let p = write_temp("wedges.json", "[[\"a\", \"b\", 2], [\"b\", \"c\", 3]]");
        let dir = p.parent().unwrap().to_path_buf();
        let out = run_with_inputs(
            "pred edge(node, node): Trop.\n\
             pred reach(node, node): Trop.\n\
             reach(X, Y) :- edge(X, Y).\n\
             reach(X, Z) :- edge(X, Y), reach(Y, Z).\n\
             input edge from \"wedges.json\".\n",
            &dir,
        );
        assert!(out.contains("reach(a, c) = 5"), "{out}");
    }

    #[test]
    fn soft_input_feeds_a_neural_predicate() {
        // The model's outputs materialized to a file: rows carry a trailing
        // probability and land in the probabilistic EDB — E1010 stays closed
        // for certain rows (missing column fails at load).
        let p = write_temp("flags.tsv", "acme\t0.9\nglobex\t0.2\n");
        let dir = p.parent().unwrap().to_path_buf();
        let src = "\
domain firm.
neural flag(firm) from model \"aml_gnn\".
pred investigate(firm): Bool.
investigate(X) :- flag(X).
input flag from \"flags.tsv\".
?prob investigate(acme).
";
        let out = run_with_inputs(src, &dir);
        // World summation carries float noise (0.72 + 0.18): compare numerically.
        let p: f64 = out.split_whitespace().next().unwrap().parse().unwrap();
        assert!((p - 0.9).abs() < 1e-12, "{out}");
        assert!(out.contains(":: investigate(acme)"), "{out}");

        // A certain row (no probability column) is a load error, not silence.
        let bad = write_temp("flags_bad.tsv", "acme\n");
        let dir = bad.parent().unwrap().to_path_buf();
        let (prog, _) = parse(&src.replace("flags.tsv", "flags_bad.tsv"));
        let mut checked = check_program(&prog).expect("check");
        let err =
            load_inputs(&prog, &mut checked, &dir).expect_err("certain row on neural must fail");
        assert!(err.contains("column"), "{err}");
    }

    #[test]
    fn tsv_edb_loading() {
        // Write an edges.tsv next to a program that reads it via `input`.
        let dir = std::env::temp_dir().join("strata_cli5_tsv_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("edges.tsv"), "a\tb\nb\tc\nc\td\n").unwrap();

        let src = "\
pred edge(node, node): Bool.
pred path(node, node): Bool.
input edge from \"edges.tsv\".
path(X, Y) :- edge(X, Y).
path(X, Z) :- edge(X, Y), path(Y, Z).
";
        let (prog, diags) = parse(src);
        assert!(!diags.has_errors(), "{}", diags.render_text(src));
        let mut checked = check_program(&prog).expect("check");
        load_inputs(&prog, &mut checked, &dir).expect("load");
        let facts = checked.edb.clone();
        let out = run_program(
            &checked.core,
            &checked.dict,
            &facts,
            false,
            &mut checked.terms,
        )
        .expect("run");

        // transitive closure computed from the TSV edges
        assert!(out.contains("path(a, d)"), "{out}");
        assert!(out.contains("edge(c, d)"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
