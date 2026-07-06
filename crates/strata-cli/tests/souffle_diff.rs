//! Differential test: our engine vs Soufflé on the Bool fragment. [INFRA-3/4/11, D13]
//!
//! Translates a checked Bool Core-IR program to a Soufflé `.dl` program + `.facts`
//! files, runs `souffle`, and asserts every output relation matches our reference
//! interpreter's, per predicate, as sorted tuple sets. Trop is out of scope here
//! (checked by an independent SSSP oracle, D13). Skips gracefully when `souffle`
//! is not installed (INFRA-11).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use strata_check::{check_program, Checked};
use strata_eval::{run, Ann, GroundVal};
use strata_front::parse;
use strata_ir::core::{CoreAtom, CoreLiteral, CorePred, CoreProgram, CoreRule, CoreTerm, Semiring};
use strata_ir::dict::{SymbolDict, SymbolId};
use strata_ir::value::GroundFact;

/// Translate a Bool Core-IR program to Soufflé source, or `None` if it uses
/// features outside the Soufflé-comparable Bool fragment (Trop / Int / aggregate).
fn to_datalog(
    core: &CoreProgram,
    dict: &SymbolDict,
    edb_preds: &BTreeSet<String>,
) -> Option<String> {
    if core.predicates.iter().any(|p| p.semiring != Semiring::Bool) {
        return None;
    }
    let mut s = String::new();
    for p in &core.predicates {
        let cols: Vec<String> = (0..p.arity).map(|i| format!("x{i}:symbol")).collect();
        let _ = writeln!(s, ".decl {}({})", p.name, cols.join(", "));
    }
    for p in edb_preds {
        let _ = writeln!(s, ".input {p}");
    }
    for p in &core.predicates {
        let _ = writeln!(s, ".output {}", p.name);
    }
    for r in &core.rules {
        let head = atom_to_dl(&r.head, dict)?;
        let mut lits = Vec::new();
        for lit in &r.body {
            lits.push(match lit {
                CoreLiteral::Pos(a) => atom_to_dl(a, dict)?,
                CoreLiteral::Neg(a) => format!("!{}", atom_to_dl(a, dict)?),
            });
        }
        let _ = writeln!(s, "{head} :- {}.", lits.join(", "));
    }
    Some(s)
}

fn atom_to_dl(a: &CoreAtom, dict: &SymbolDict) -> Option<String> {
    let mut args = Vec::new();
    for t in &a.args {
        args.push(match t {
            CoreTerm::Var { slot } => format!("v{slot}"),
            CoreTerm::Const { sym } => format!("{:?}", dict.resolve(*sym)?), // quoted symbol
            // Int / aggregate / compound terms are outside the Soufflé-comparable
            // Bool fragment.
            CoreTerm::Int { .. } | CoreTerm::Agg { .. } | CoreTerm::Compound { .. } => return None,
        });
    }
    Some(format!("{}({})", a.pred, args.join(", ")))
}

/// Our engine's result as {pred -> sorted set of string tuples}.
fn our_relations(checked: &Checked) -> BTreeMap<String, BTreeSet<Vec<String>>> {
    let edb: Vec<(&str, Vec<GroundVal>, Ann)> = checked
        .edb
        .iter()
        .map(|f| (f.pred.as_str(), f.args.clone(), Ann::from_weight(f.weight)))
        .collect();
    let db = run(&checked.core, &edb).expect("eval");
    let mut out = BTreeMap::new();
    for p in &checked.core.predicates {
        let rel = db.relation(&p.name).unwrap();
        let tuples: BTreeSet<Vec<String>> = rel
            .rows
            .keys()
            .map(|t| t.iter().map(|v| resolve(v, &checked.dict)).collect())
            .collect();
        out.insert(p.name.clone(), tuples);
    }
    out
}

fn resolve(v: &GroundVal, dict: &SymbolDict) -> String {
    match v {
        GroundVal::Sym(id) => dict.resolve(*id).unwrap_or("?").to_string(),
        GroundVal::Int(n) => n.to_string(),
        GroundVal::Term(id) => format!("<term#{}>", id.0), // @terms are outside the Bool fragment
    }
}

/// Run souffle over the checked program; `None` if souffle is unavailable or the
/// program is not Bool-comparable.
fn souffle_relations(
    checked: &Checked,
    dir: &Path,
) -> Option<BTreeMap<String, BTreeSet<Vec<String>>>> {
    let edb_preds: BTreeSet<String> = checked.edb.iter().map(|f| f.pred.clone()).collect();
    let dl = to_datalog(&checked.core, &checked.dict, &edb_preds)?;

    std::fs::create_dir_all(dir).ok()?;
    std::fs::write(dir.join("prog.dl"), dl).ok()?;
    // Write <pred>.facts for each EDB predicate.
    let mut facts: BTreeMap<&str, String> = BTreeMap::new();
    for f in &checked.edb {
        let row: Vec<String> = f.args.iter().map(|v| resolve(v, &checked.dict)).collect();
        let e = facts.entry(f.pred.as_str()).or_default();
        e.push_str(&row.join("\t"));
        e.push('\n');
    }
    for pred in &edb_preds {
        let content = facts.get(pred.as_str()).cloned().unwrap_or_default();
        std::fs::write(dir.join(format!("{pred}.facts")), content).ok()?;
    }

    let out = Command::new("souffle")
        .arg("-F")
        .arg(dir)
        .arg("-D")
        .arg(dir)
        .arg(dir.join("prog.dl"))
        .output()
        .ok()?; // souffle not installed → skip
    if !out.status.success() {
        panic!("souffle failed: {}", String::from_utf8_lossy(&out.stderr));
    }

    let mut result = BTreeMap::new();
    for p in &checked.core.predicates {
        let csv = std::fs::read_to_string(dir.join(format!("{}.csv", p.name))).unwrap_or_default();
        let tuples: BTreeSet<Vec<String>> = csv
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.split('\t').map(String::from).collect())
            .collect();
        result.insert(p.name.clone(), tuples);
    }
    Some(result)
}

/// A per-process scratch dir so concurrent `cargo test` runs never collide on
/// souffle's `.dl` / `.facts` / `.csv` files.
fn scratch(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("strata_souffle_{}_{name}", std::process::id()))
}

/// Under `STRATA_REQUIRE_ORACLES` (the oracle CI job), a missing external
/// oracle is a hard failure — the differential must actually run. Locally,
/// absence skips cleanly (INFRA-11).
fn skip_or_die(what: &str) {
    if std::env::var_os("STRATA_REQUIRE_ORACLES").is_some() {
        panic!("{what} — but STRATA_REQUIRE_ORACLES is set, the oracle differential must run");
    }
    eprintln!("skipping: {what}");
}

fn diff(name: &str, src: &str) {
    let (prog, diags) = parse(src);
    assert!(!diags.has_errors(), "{}", diags.render_text(src));
    let checked = check_program(&prog).expect("check");

    let dir = scratch(name);
    let _ = std::fs::remove_dir_all(&dir);
    let Some(souffle) = souffle_relations(&checked, &dir) else {
        skip_or_die(&format!(
            "{name}: souffle unavailable or program not Bool-comparable"
        ));
        return;
    };
    let ours = our_relations(&checked);
    assert!(
        ours.values().any(|s| !s.is_empty()),
        "vacuous diff for `{name}`: no tuples produced"
    );
    assert_eq!(ours, souffle, "engine and Soufflé disagree on `{name}`");
    let _ = std::fs::remove_dir_all(&dir);
}

const TC: &str = "\
pred edge(node, node): Bool.
pred path(node, node): Bool.
edge(a, b).
edge(b, c).
edge(c, d).
path(X, Y) :- edge(X, Y).
path(X, Z) :- edge(X, Y), path(Y, Z).
";

const SAME_GENERATION: &str = "\
pred par(node, node): Bool.
pred sg(node, node): Bool.
par(a, b).
par(a, c).
par(b, d).
par(c, e).
sg(X, Y) :- par(Z, X), par(Z, Y).
sg(X, Y) :- par(P, X), sg(P, Q), par(Q, Y).
";

const STRATIFIED_NEGATION: &str = "\
pred node(node): Bool.
pred edge(node, node): Bool.
pred reach(node, node): Bool.
pred noreach(node, node): Bool.
node(a).
node(b).
node(c).
edge(a, b).
edge(b, c).
reach(X, Y) :- edge(X, Y).
reach(X, Z) :- edge(X, Y), reach(Y, Z).
noreach(X, Y) :- node(X), node(Y), not reach(X, Y).
";

// --- fuzzer: random Bool programs vs Soufflé [INFRA-5, D13] --------------------

/// Tiny deterministic xorshift PRNG (seed = repro).
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(0x1234_5678))
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo + 1)
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.next_u64() % 100 < pct
    }
}

/// Build a random safe, stratifiable, Soufflé-comparable Bool program as a
/// `Checked` (symbol constants only — no Int/Trop/aggregate).
fn gen_bool(seed: u64) -> Checked {
    let mut r = Rng::new(seed);
    let domain = r.range(2, 4);
    let n_pred = r.range(2, 3);
    let vars = r.range(2, 3);

    let mut dict = SymbolDict::new();
    for i in 0..domain {
        dict.intern(&format!("c{i}"));
    }
    let pname = |i: usize| format!("p{i}");
    let mut predicates: Vec<CorePred> = (0..n_pred)
        .map(|i| CorePred {
            name: pname(i),
            arity: 2,
            semiring: Semiring::Bool,
            stratum: 0,
        })
        .collect();

    let mut rules: Vec<CoreRule> = Vec::new();
    for _ in 0..r.range(2, 5) {
        let mut body = Vec::new();
        let mut body_vars: Vec<u32> = Vec::new();
        for _ in 0..r.range(1, 3) {
            let a = r.below(vars) as u32;
            let b = r.below(vars) as u32;
            body_vars.push(a);
            body_vars.push(b);
            body.push(CoreLiteral::Pos(CoreAtom {
                pred: pname(r.below(n_pred)),
                args: vec![CoreTerm::Var { slot: a }, CoreTerm::Var { slot: b }],
            }));
        }
        let h1 = body_vars[r.below(body_vars.len())];
        let h2 = body_vars[r.below(body_vars.len())];
        rules.push(CoreRule {
            head: CoreAtom {
                pred: pname(r.below(n_pred)),
                args: vec![CoreTerm::Var { slot: h1 }, CoreTerm::Var { slot: h2 }],
            },
            body,
            stratum: 0,
            var_count: vars as u32,
            neg_weight_cycle_check: false,
        });
    }

    let mut num_strata = 1;
    // Optionally add a stratum-1 predicate with a negated stratum-0 literal
    // (guaranteed stratifiable — Soufflé accepts it).
    if n_pred >= 2 && r.chance(40) {
        predicates.push(CorePred {
            name: pname(n_pred),
            arity: 2,
            semiring: Semiring::Bool,
            stratum: 1,
        });
        rules.push(CoreRule {
            head: CoreAtom {
                pred: pname(n_pred),
                args: vec![CoreTerm::Var { slot: 0 }, CoreTerm::Var { slot: 1 }],
            },
            body: vec![
                CoreLiteral::Pos(CoreAtom {
                    pred: pname(r.below(n_pred)),
                    args: vec![CoreTerm::Var { slot: 0 }, CoreTerm::Var { slot: 1 }],
                }),
                CoreLiteral::Neg(CoreAtom {
                    pred: pname(r.below(n_pred)),
                    args: vec![CoreTerm::Var { slot: 0 }, CoreTerm::Var { slot: 1 }],
                }),
            ],
            stratum: 1,
            var_count: 2,
            neg_weight_cycle_check: false,
        });
        num_strata = 2;
    }

    let mut edb = Vec::new();
    for _ in 0..r.range(2, 8) {
        let a = GroundVal::Sym(SymbolId(r.below(domain) as u32));
        let b = GroundVal::Sym(SymbolId(r.below(domain) as u32));
        edb.push(GroundFact {
            pred: pname(r.below(n_pred)),
            args: vec![a, b],
            weight: None,
        });
    }

    Checked {
        core: CoreProgram {
            predicates,
            rules,
            num_strata,
        },
        dict,
        edb,
        prob_edb: Vec::new(),
        queries: Vec::new(),
        neural: Vec::new(),
        terms: strata_ir::terms::TermTable::new(strata_ir::terms::DEFAULT_MAX_DEPTH),
        annotations: std::collections::HashMap::new(),
    }
}

#[test]
fn fuzz_bool_vs_souffle() {
    // Soufflé differential half of the "10k fuzz" exit item (INFRA-5). Each
    // program spawns a souffle process (~46ms), so the default count is modest;
    // set STRATA_SOUFFLE_FUZZ_N for a full run (e.g. 10000). Skips if souffle is
    // absent (INFRA-11).
    let n: u64 = std::env::var("STRATA_SOUFFLE_FUZZ_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let dir = scratch("fuzz");

    // Probe once so absence of souffle skips cleanly instead of looping.
    if souffle_relations(&gen_bool(0), &dir).is_none() {
        skip_or_die("fuzz_bool_vs_souffle: souffle unavailable");
        return;
    }
    for seed in 0..n {
        let checked = gen_bool(seed);
        let _ = std::fs::remove_dir_all(&dir);
        let souffle = souffle_relations(&checked, &dir).expect("souffle");
        let ours = our_relations(&checked);
        assert_eq!(ours, souffle, "engine and Soufflé disagree at seed {seed}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn diff_transitive_closure() {
    diff("tc", TC);
}

#[test]
fn diff_same_generation() {
    diff("same_generation", SAME_GENERATION);
}

#[test]
fn diff_stratified_negation() {
    diff("stratified_negation", STRATIFIED_NEGATION);
}
