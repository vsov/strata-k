//! `strata` — the single user-facing binary. [CLI-1/2/3/4/6/7/8, D10]
//!
//! Subcommands: `check | run | fmt | ir`, with a global `--error-format=text|json`
//! (D9). Ties strata-front + strata-check + strata-eval into the end-to-end path
//! text → parse → check → Core-IR → interpret → result.

use std::path::Path;
use std::process::ExitCode as ProcExit;

use strata_check::{check_program, Checked};
use strata_eval::{marginals, prob, run_semi_naive, run_terms, Ann, Db, GroundVal};
use strata_front::{format, parse, print_program};
use strata_ir::core::{CoreProgram, Semiring};
use strata_ir::dict::SymbolDict;
use strata_ir::high::program::{ItemKind, Pragma, QueryKind, Term};
use strata_ir::high::Program;
use strata_ir::terms::TermTable;
use strata_ir::trop::Weight;
use strata_ir::value::GroundFact;

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
        // @asp uses stable-model semantics (unstratified negation allowed), so it
        // bypasses the stratifying checker; parse-level well-formedness suffices.
        if json {
            println!("[]");
        } else {
            eprintln!("ok (asp)");
        }
        return Code::Ok;
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
        // ASP (Phase 5): compute stable models via the reference solver.
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
    let mut facts = checked.edb.clone();
    match load_inputs(&prog, &checked.core, &mut checked.dict, base) {
        Ok(loaded) => facts.extend(loaded),
        Err(e) => {
            eprintln!("strata: {e}");
            return Code::Usage;
        }
    }

    // режим B: a `?grad` query differentiates the marginal; a `?prob` query (or
    // any probabilistic fact) asks for the marginal itself (Phase 4). Otherwise a
    // plain evaluation.
    let result = if grad_mode(&checked) {
        run_grad(&checked, &facts)
    } else if prob_mode(&checked) {
        run_prob(&checked, &facts)
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

fn prob_mode(checked: &Checked) -> bool {
    !checked.prob_edb.is_empty() || checked.queries.iter().any(|q| q.kind == QueryKind::Prob)
}

/// Answer probabilistic queries (or dump all marginals) via режим B. [Phase 4]
fn run_prob(checked: &Checked, certain_facts: &[GroundFact]) -> Result<String, String> {
    let certain: Vec<(String, Vec<GroundVal>)> = certain_facts
        .iter()
        .map(|f| (f.pred.clone(), f.args.clone()))
        .collect();
    let prob_edb = &checked.prob_edb;
    let dict = &checked.dict;

    let prob_qs: Vec<_> = checked
        .queries
        .iter()
        .filter(|q| q.kind == QueryKind::Prob)
        .collect();
    let mut out = String::new();

    if prob_qs.is_empty() {
        // No explicit query: print every predicate's marginal, sorted.
        let m = marginals(&checked.core, &certain, prob_edb).map_err(|e| e.to_string())?;
        let mut preds: Vec<&String> = m.keys().collect();
        preds.sort();
        for pred in preds {
            let mut tuples: Vec<(&Vec<GroundVal>, &f64)> = m[pred].iter().collect();
            tuples.sort_by(|a, b| a.0.cmp(b.0));
            for (tuple, p) in tuples {
                out.push_str(&prob_line(*p, pred, tuple, dict));
            }
        }
    } else {
        for q in prob_qs {
            let ans = prob::query(&checked.core, &certain, prob_edb, &q.pred, &q.pattern)
                .map_err(|e| e.to_string())?;
            for (tuple, p) in ans {
                out.push_str(&prob_line(p, &q.pred, &tuple, dict));
            }
        }
    }
    Ok(out)
}

fn prob_line(p: f64, pred: &str, tuple: &[GroundVal], dict: &SymbolDict) -> String {
    let args: Vec<String> = tuple.iter().map(|v| render_val(v, dict)).collect();
    format!("{p} :: {pred}({})\n", args.join(", "))
}

fn grad_mode(checked: &Checked) -> bool {
    checked.queries.iter().any(|q| q.kind == QueryKind::Grad)
}

/// Answer `?grad` queries: the marginal probability of each matching tuple and
/// its gradient w.r.t. every probabilistic fact's probability (reverse-mode over
/// the режим-B chain, spec §2.3). [gradient wiring]
fn run_grad(checked: &Checked, certain_facts: &[GroundFact]) -> Result<String, String> {
    let certain: Vec<(String, Vec<GroundVal>)> = certain_facts
        .iter()
        .map(|f| (f.pred.clone(), f.args.clone()))
        .collect();
    let prob_edb = &checked.prob_edb;
    let dict = &checked.dict;

    let mut out = String::new();
    for q in checked.queries.iter().filter(|q| q.kind == QueryKind::Grad) {
        let ans = prob::grad_query(&checked.core, &certain, prob_edb, &q.pred, &q.pattern)
            .map_err(|e| e.to_string())?;
        for (tuple, p, grad) in ans {
            out.push_str(&prob_line(p, &q.pred, &tuple, dict));
            // one gradient line per probabilistic fact, labelled by that fact;
            // a neural fact also names the model the gradient backpropagates into.
            for ((pred, ptuple, pw), g) in prob_edb.iter().zip(&grad) {
                let args: Vec<String> = ptuple.iter().map(|v| render_val(v, dict)).collect();
                let model = checked
                    .neural
                    .iter()
                    .find(|(n, _)| n == pred)
                    .map(|(_, m)| format!("  (→ model {m:?})"))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "  ∂/∂[{pw} :: {pred}({})] = {g}{model}\n",
                    args.join(", ")
                ));
            }
        }
    }
    Ok(out)
}

// --- ASP (Phase 5) -----------------------------------------------------------

fn is_asp(prog: &Program) -> bool {
    prog.items
        .iter()
        .any(|i| matches!(&i.node, ItemKind::Pragma(Pragma::Asp)))
}

/// Enumerate the stable models of an `@asp` program via the reference solver.
fn run_asp(prog: &Program) -> Result<String, String> {
    use strata_asp::Val;

    let mut rules = Vec::new();
    let mut facts: Vec<(String, Vec<Val>)> = Vec::new();
    for item in &prog.items {
        match &item.node {
            ItemKind::Rule(r) => rules.push(r.clone()),
            ItemKind::Fact(f) => {
                let mut args = Vec::with_capacity(f.atom.args.len());
                let mut ground = true;
                for t in &f.atom.args {
                    match t {
                        Term::Const { name } => args.push(Val::Sym(name.clone())),
                        Term::Int { value } => args.push(Val::Int(*value)),
                        _ => ground = false,
                    }
                }
                if ground {
                    facts.push((f.atom.pred.clone(), args));
                }
            }
            _ => {}
        }
    }

    let models = strata_asp::solve(&rules, &facts, &[]).map_err(|e| e.to_string())?;
    if models.is_empty() {
        return Ok("UNSATISFIABLE\n".to_string());
    }
    let mut out = String::new();
    for (i, m) in models.iter().enumerate() {
        let atoms: Vec<String> = m.iter().map(render_asp_atom).collect();
        out.push_str(&format!("Answer {}: {{{}}}\n", i + 1, atoms.join(", ")));
    }
    Ok(out)
}

fn render_asp_atom((pred, args): &(String, Vec<strata_asp::Val>)) -> String {
    if args.is_empty() {
        pred.clone()
    } else {
        let rendered: Vec<String> = args.iter().map(render_asp_val).collect();
        format!("{pred}({})", rendered.join(", "))
    }
}

fn render_asp_val(v: &strata_asp::Val) -> String {
    match v {
        strata_asp::Val::Sym(s) => s.clone(),
        strata_asp::Val::Int(n) => n.to_string(),
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
    let db = if semi_naive {
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

/// Load every `input pred from "file.tsv"` declaration's EDB (CLI-5, D10).
///
/// TSV convention (Soufflé-compatible): one row per fact, tab-separated columns
/// interned as symbol constants; a `Trop` predicate has one extra trailing
/// integer weight column. Constants are interned into `dict` so they align with
/// the constants the checker already lowered.
fn load_inputs(
    program: &Program,
    core: &CoreProgram,
    dict: &mut SymbolDict,
    base: &Path,
) -> Result<Vec<GroundFact>, String> {
    let mut out = Vec::new();
    for item in &program.items {
        let ItemKind::Input(inp) = &item.node else {
            continue;
        };
        let pred = core
            .predicates
            .iter()
            .find(|p| p.name == inp.pred)
            .ok_or_else(|| format!("input predicate `{}` is not declared/executable", inp.pred))?;
        let is_trop = pred.semiring == Semiring::Trop;
        let ncols = pred.arity as usize + usize::from(is_trop);
        let path = base.join(&inp.path);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        for (i, line) in text.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() != ncols {
                return Err(format!(
                    "{}:{}: `{}` expects {ncols} column(s), found {}",
                    path.display(),
                    i + 1,
                    inp.pred,
                    cols.len()
                ));
            }
            let args = cols[..pred.arity as usize]
                .iter()
                .map(|c| GroundVal::Sym(dict.intern(c)))
                .collect();
            let weight = if is_trop {
                let raw = cols[pred.arity as usize];
                Some(Weight::Finite(raw.parse::<i64>().map_err(|_| {
                    format!("{}:{}: bad weight {raw:?}", path.display(), i + 1)
                })?))
            } else {
                None
            };
            out.push(GroundFact {
                pred: inp.pred.clone(),
                args,
                weight,
            });
        }
    }
    Ok(out)
}

/// Canonical output: relations in name order, tuples sorted (BTreeMap), constants
/// resolved through the dictionary (IR-10). [render half of IR-10]
fn render_db(db: &Db, dict: &SymbolDict, terms: &TermTable) -> String {
    let mut out = String::new();
    for pred in db.predicates() {
        let rel = db.relation(pred).unwrap();
        for (tuple, ann) in &rel.rows {
            let args: Vec<String> = tuple.iter().map(|v| render_val_t(v, dict, terms)).collect();
            out.push_str(&format!("{pred}({})", args.join(", ")));
            if let Ann::W(w) = ann {
                out.push_str(&format!(" = {}", render_weight(*w)));
            }
            out.push('\n');
        }
    }
    out
}

/// Render a ground value; compound terms (`@terms`) are reconstructed structurally
/// from the term table.
fn render_val_t(v: &GroundVal, dict: &SymbolDict, terms: &TermTable) -> String {
    match v {
        GroundVal::Sym(id) => dict.resolve(*id).unwrap_or("?").to_string(),
        GroundVal::Int(n) => n.to_string(),
        GroundVal::Term(id) => {
            let (functor, args) = terms.get(*id);
            let inner: Vec<String> = args.iter().map(|a| render_val_t(a, dict, terms)).collect();
            format!(
                "{}({})",
                dict.resolve(functor).unwrap_or("?"),
                inner.join(", ")
            )
        }
    }
}

/// Term-free render (probabilistic / gradient paths never produce `@terms`).
fn render_val(v: &GroundVal, dict: &SymbolDict) -> String {
    match v {
        GroundVal::Sym(id) => dict.resolve(*id).unwrap_or("?").to_string(),
        GroundVal::Int(n) => n.to_string(),
        GroundVal::Term(id) => format!("<term#{}>", id.0),
    }
}

fn render_weight(w: Weight) -> String {
    match w {
        Weight::Finite(n) => n.to_string(),
        Weight::PosInf => strata_ir::output::POS_INF_TOKEN.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let checked = check_program(&prog).expect("check");
        assert!(prob_mode(&checked));
        let out = run_prob(&checked, &checked.edb).expect("prob run");
        assert_eq!(out, "0.625 :: path(a, c)\n", "{out}");
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
        let checked = check_program(&prog).expect("check");
        assert!(grad_mode(&checked));
        let out = run_grad(&checked, &checked.edb).expect("grad run");
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
        let out = run_grad(&checked, &checked.edb).expect("neural grad run");
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
        let loaded = load_inputs(&prog, &checked.core, &mut checked.dict, &dir).expect("load");
        let mut facts = checked.edb.clone();
        facts.extend(loaded);
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
