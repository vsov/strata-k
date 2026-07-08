//! GPU vs CPU race on the same workload. [verification — perf, not oracle]
//!
//! Runs the bench graph (K disjoint C-chains, closure exactly known) through
//! BOTH engines — the GPU kernels and `strata-eval`'s semi-naive CPU engine —
//! and reports wall-clock for each plus the ratio. Sizes must match on both
//! sides or the run aborts, so a speedup can never be quoted off a wrong
//! answer. Size is env-tunable via `STRATA_BENCH_K` / `STRATA_BENCH_C`.
//!
//!   cargo run -p strata-gpu --example race --features cuda --release
//!
//! The dev-dependency on `strata-eval` is for THIS race only: the crate's
//! differential tests keep their independent in-crate oracles (the GPU is
//! never checked against the code it is racing).

#[cfg(not(feature = "cuda"))]
fn main() {
    eprintln!("race needs --features cuda (an NVIDIA GPU + CUDA toolkit)");
}

#[cfg(feature = "cuda")]
fn main() {
    use std::time::Instant;
    use strata_eval::{run_semi_naive, Ann, GroundVal};
    use strata_gpu::{shortest_paths_trop, transitive_closure_bool};
    use strata_ir::core::{
        CoreAtom, CoreLiteral, CorePred, CoreProgram, CoreRule, CoreTerm, Semiring,
    };
    use strata_ir::dict::SymbolId;
    use strata_ir::trop::Weight;

    let env = |k: &str, d: u32| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let k: u32 = env("STRATA_BENCH_K", 200_000);
    let c: u32 = env("STRATA_BENCH_C", 5);
    let stride = c + 1;

    let mut edges: Vec<(u32, u32)> = Vec::with_capacity((k * c) as usize);
    for chain in 0..k {
        let base = chain * stride;
        for i in 0..c {
            edges.push((base + i, base + i + 1));
        }
    }
    let expected = k as usize * (c as usize * (c as usize + 1) / 2);
    println!(
        "graph: {} disjoint {}-chains → {} edges, closure {} pairs",
        k,
        c,
        edges.len(),
        expected
    );

    // The same TC program the CPU engine sees everywhere else: edge EDB,
    // path(X,Y) :- edge(X,Y). path(X,Z) :- edge(X,Y), path(Y,Z).
    let var = |slot: u32| CoreTerm::Var { slot };
    let atom = |pred: &str, a: u32, b: u32| CoreAtom {
        pred: pred.to_string(),
        args: vec![var(a), var(b)],
    };
    let program = |sem: Semiring| CoreProgram {
        predicates: vec![
            CorePred {
                name: "edge".into(),
                arity: 2,
                semiring: sem,
                stratum: 0,
            },
            CorePred {
                name: "path".into(),
                arity: 2,
                semiring: sem,
                stratum: 0,
            },
        ],
        rules: vec![
            CoreRule {
                head: atom("path", 0, 1),
                body: vec![CoreLiteral::Pos(atom("edge", 0, 1))],
                stratum: 0,
                var_count: 2,
                neg_weight_cycle_check: false,
            },
            CoreRule {
                head: atom("path", 0, 2),
                body: vec![
                    CoreLiteral::Pos(atom("edge", 0, 1)),
                    CoreLiteral::Pos(atom("path", 1, 2)),
                ],
                stratum: 0,
                var_count: 3,
                neg_weight_cycle_check: false,
            },
        ],
        num_strata: 1,
    };

    let race = |name: &str, sem: Semiring, gpu_pairs: usize, gpu_dt: std::time::Duration| {
        let ann = match sem {
            Semiring::Trop => Ann::W(Weight::Finite(1)),
            _ => Ann::Unit,
        };
        let edb: Vec<(&str, Vec<GroundVal>, Ann)> = edges
            .iter()
            .map(|&(a, b)| {
                (
                    "edge",
                    vec![GroundVal::Sym(SymbolId(a)), GroundVal::Sym(SymbolId(b))],
                    ann,
                )
            })
            .collect();
        let prog = program(sem);
        let t = Instant::now();
        let db = run_semi_naive(&prog, &edb).expect("cpu semi-naive");
        let cpu_dt = t.elapsed();
        let cpu_pairs = db.relation("path").map(|r| r.len()).unwrap_or(0);
        assert_eq!(cpu_pairs, expected, "{name}: CPU closure size mismatch");
        assert_eq!(gpu_pairs, expected, "{name}: GPU closure size mismatch");
        println!(
            "{name}:  GPU {gpu_dt:?}  |  CPU semi-naive {cpu_dt:?}  |  ratio {:.1}x",
            cpu_dt.as_secs_f64() / gpu_dt.as_secs_f64()
        );
    };

    // --- Bool ---
    let t = Instant::now();
    let tc = transitive_closure_bool(&edges).expect("gpu tc");
    let gpu_bool = t.elapsed();
    race("Bool TC ", Semiring::Bool, tc.len(), gpu_bool);
    drop(tc);

    // --- Trop (unit weights; closure support identical to Bool) ---
    let weighted: Vec<(u32, u32, i64)> = edges.iter().map(|&(a, b)| (a, b, 1)).collect();
    let t = Instant::now();
    let sp = shortest_paths_trop(&weighted).expect("gpu trop");
    let gpu_trop = t.elapsed();
    race("Trop SP ", Semiring::Trop, sp.len(), gpu_trop);
}
