//! Property fuzzer: naive == semi-naive on thousands of random programs.
//! [INFRA-5/7 without Soufflé — the engine differential, D7/D15]
//!
//! Generates random, safe-by-construction Core-IR programs (positive recursive
//! Bool/Trop, plus stratified Bool negation) over a tiny domain and asserts the
//! naive oracle and the semi-naive engine agree bit-for-bit. A mismatch prints
//! the seed for a deterministic repro. Soufflé is absent in this environment, so
//! the external oracle (INFRA-3/4) is out of scope here; this is the internal
//! naive-vs-semi-naive gate (I5), which also stress-tests the delta logic.

use strata_eval::{run, run_semi_naive, Ann, GroundVal};
use strata_ir::core::{CoreAtom, CoreLiteral, CorePred, CoreProgram, CoreRule, CoreTerm, Semiring};
use strata_ir::dict::SymbolId;
use strata_ir::trop::Weight;

/// Tiny deterministic xorshift PRNG (no deps; seedable for repro).
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        // Map the seed to a nonzero state (xorshift stalls on 0).
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

type EdbOwned = Vec<(String, Vec<GroundVal>, Ann)>;

/// Build a random safe Core-IR program + EDB for `seed`.
fn generate(seed: u64) -> (CoreProgram, EdbOwned) {
    let mut r = Rng::new(seed);
    let trop = r.chance(50);
    let sem = if trop { Semiring::Trop } else { Semiring::Bool };
    let domain = r.range(2, 4); // # constants
    let n_pred = r.range(2, 3); // # stratum-0 IDB preds
    let vars = r.range(2, 3); // variable slots per rule

    let pname = |i: usize| format!("p{i}");
    let mut predicates: Vec<CorePred> = (0..n_pred)
        .map(|i| CorePred {
            name: pname(i),
            arity: 2,
            semiring: sem,
            stratum: 0,
        })
        .collect();

    // Random positive rules (safe: head vars are drawn from body vars).
    let mut rules: Vec<CoreRule> = Vec::new();
    let n_rules = r.range(2, 5);
    for _ in 0..n_rules {
        let n_body = r.range(1, 3);
        let mut body = Vec::new();
        let mut body_vars: Vec<u32> = Vec::new();
        for _ in 0..n_body {
            let p = r.below(n_pred);
            let a = r.below(vars) as u32;
            let b = r.below(vars) as u32;
            body_vars.push(a);
            body_vars.push(b);
            body.push(CoreLiteral::Pos(CoreAtom {
                pred: pname(p),
                args: vec![CoreTerm::Var { slot: a }, CoreTerm::Var { slot: b }],
            }));
        }
        let hv1 = body_vars[r.below(body_vars.len())];
        let hv2 = body_vars[r.below(body_vars.len())];
        rules.push(CoreRule {
            head: CoreAtom {
                pred: pname(r.below(n_pred)),
                args: vec![CoreTerm::Var { slot: hv1 }, CoreTerm::Var { slot: hv2 }],
            },
            body,
            stratum: 0,
            var_count: vars as u32,
            neg_weight_cycle_check: trop,
        });
    }

    let mut num_strata = 1;
    // Optionally add a stratum-1 predicate defined with a negated stratum-0 literal
    // (Bool only — Trop negation is out of Phase-0 scope).
    if !trop && n_pred >= 2 && r.chance(35) {
        let q = pname(n_pred);
        predicates.push(CorePred {
            name: q.clone(),
            arity: 2,
            semiring: Semiring::Bool,
            stratum: 1,
        });
        let p = r.below(n_pred);
        let neg = r.below(n_pred);
        rules.push(CoreRule {
            head: CoreAtom {
                pred: q,
                args: vec![CoreTerm::Var { slot: 0 }, CoreTerm::Var { slot: 1 }],
            },
            body: vec![
                CoreLiteral::Pos(CoreAtom {
                    pred: pname(p),
                    args: vec![CoreTerm::Var { slot: 0 }, CoreTerm::Var { slot: 1 }],
                }),
                CoreLiteral::Neg(CoreAtom {
                    pred: pname(neg),
                    args: vec![CoreTerm::Var { slot: 0 }, CoreTerm::Var { slot: 1 }],
                }),
            ],
            stratum: 1,
            var_count: 2,
            neg_weight_cycle_check: false,
        });
        num_strata = 2;
    }

    // Random EDB over the stratum-0 predicates.
    let n_facts = r.range(2, 8);
    let mut edb: EdbOwned = Vec::new();
    for _ in 0..n_facts {
        let p = pname(r.below(n_pred));
        let a = GroundVal::Sym(SymbolId(r.below(domain) as u32));
        let b = GroundVal::Sym(SymbolId(r.below(domain) as u32));
        let ann = if trop {
            Ann::W(Weight::Finite(r.range(0, 4) as i64))
        } else {
            Ann::Unit
        };
        edb.push((p, vec![a, b], ann));
    }

    (
        CoreProgram {
            predicates,
            rules,
            num_strata,
        },
        edb,
    )
}

#[test]
fn naive_equals_semi_naive_over_random_programs() {
    // D15 exit-item scale (10k). This is the engine differential; the Soufflé
    // differential (INFRA-3/4/5) is added once a pinned Soufflé exists (INFRA-11).
    let n = 10_000;
    for seed in 0..n {
        let (prog, edb_owned) = generate(seed);
        let edb: Vec<(&str, Vec<GroundVal>, Ann)> = edb_owned
            .iter()
            .map(|(p, a, an)| (p.as_str(), a.clone(), *an))
            .collect();

        let naive = run(&prog, &edb);
        let semi = run_semi_naive(&prog, &edb);
        match (naive, semi) {
            (Ok(a), Ok(b)) => assert_eq!(a, b, "naive != semi-naive at seed {seed}"),
            (Err(a), Err(b)) => assert_eq!(a, b, "engines disagree on error at seed {seed}"),
            (a, b) => panic!("one engine errored, the other didn't at seed {seed}: {a:?} vs {b:?}"),
        }
    }
}
