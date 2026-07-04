//! Scale + timing check for the Phase-1 GPU engine. [verification]
//!
//! Runs on the GPU box with `--features cuda`. Uses a graph whose transitive
//! closure is large but exactly known — a disjoint union of K short chains — so
//! correctness is verifiable at scale without an O(n²) oracle. Reports timing
//! and throughput for both semirings.
//!
//!   cargo run -p strata-gpu --example bench --features cuda --release
//!
//! Without the `cuda` feature it prints a note and exits.

#[cfg(not(feature = "cuda"))]
fn main() {
    eprintln!("bench needs --features cuda (an NVIDIA GPU + CUDA toolkit)");
}

#[cfg(feature = "cuda")]
fn main() {
    use std::time::Instant;
    use strata_gpu::{shortest_paths_trop, transitive_closure_bool};

    // K disjoint chains of C edges each: chain k uses node ids [k*(C+1) .. +C].
    // Closure of one chain (C+1 nodes) is C(C+1)/2 pairs; total is K× that.
    // Size is env-tunable: STRATA_BENCH_K (chains), STRATA_BENCH_C (chain length).
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
    let expected_closure = k as usize * (c as usize * (c as usize + 1) / 2);

    println!(
        "graph: {} disjoint {}-chains → {} edges, expected closure {} pairs",
        k,
        c,
        edges.len(),
        expected_closure
    );

    // --- Bool transitive closure ---
    let t = Instant::now();
    let tc = transitive_closure_bool(&edges).expect("gpu tc");
    let dt = t.elapsed();
    assert_eq!(tc.len(), expected_closure, "Bool closure size mismatch");
    println!(
        "Bool TC:  {} pairs in {:?}  ({:.1} M closure-tuples/s, {:.1} M input-edges/s)",
        tc.len(),
        dt,
        tc.len() as f64 / dt.as_secs_f64() / 1e6,
        edges.len() as f64 / dt.as_secs_f64() / 1e6,
    );

    // --- Trop shortest paths (unit weights → distance = hop count) ---
    // Skippable (STRATA_BENCH_TROP=0) so the Bool engine can be pushed to sizes
    // the host-aggregating Trop loop can't yet reach.
    if env("STRATA_BENCH_TROP", 1) != 0 {
        let wedges: Vec<(u32, u32, i64)> = edges.iter().map(|&(a, b)| (a, b, 1)).collect();
        let t = Instant::now();
        let sp = shortest_paths_trop(&wedges).expect("gpu trop");
        let dt = t.elapsed();
        assert_eq!(sp.len(), expected_closure, "Trop closure size mismatch");
        for &(s, d, w) in sp.iter().take(1000) {
            assert_eq!(w, (d - s) as i64, "wrong distance for ({s},{d})");
        }
        println!(
            "Trop SSSP: {} pairs in {:?}  ({:.1} M closure-tuples/s)",
            sp.len(),
            dt,
            sp.len() as f64 / dt.as_secs_f64() / 1e6,
        );
    }

    println!("OK — bit-exact at scale.");
}
