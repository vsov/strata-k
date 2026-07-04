//! WCOJ skew demonstration. [Phase 2 exit]
//!
//! Builds a power-law graph — a few very high-degree hubs — with an exactly
//! known triangle count, then counts its triangles by GPU WCOJ. The point: the
//! binary-join intermediate (Σ_v d(v)² two-paths) is astronomically larger than
//! the graph, so a binary plan OOMs, while the leapfrog join stays O(m) and
//! finishes. Correctness is asserted against the closed-form count.
//!
//!   cargo run -p strata-gpu --example tri --features cuda --release
//!
//! Env: STRATA_TRI_HUBS, STRATA_TRI_BRIDGES, STRATA_TRI_RANDOM, STRATA_TRI_N.

#[cfg(not(feature = "cuda"))]
fn main() {
    eprintln!("tri needs --features cuda (an NVIDIA GPU + CUDA toolkit)");
}

#[cfg(feature = "cuda")]
fn main() {
    use std::time::Instant;
    use strata_gpu::{binary_plan_intermediate, count_triangles};

    let env = |k: &str, d: u64| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let h = env("STRATA_TRI_HUBS", 16) as u32;
    let b = env("STRATA_TRI_BRIDGES", 100_000) as u32;
    let d = env("STRATA_TRI_RANDOM", 500_000) as u32; // random neighbours per hub
    let n = env("STRATA_TRI_N", 10_000_000) as u32;

    // Deterministic LCG (Date/rand-free).
    let mut state = 0x1234_5678_9abc_def0u64;
    let mut rng = move |m: u32| {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((state >> 33) % m as u64) as u32
    };

    let mut edges: Vec<(u32, u32)> = Vec::new();
    // K_h among the hubs.
    for i in 0..h {
        for j in (i + 1)..h {
            edges.push((i, j));
        }
    }
    // Each of B bridge nodes (ids h..h+b) connects to every hub → forms a
    // triangle with each hub pair.
    for k in 0..b {
        let w = h + k;
        for hub in 0..h {
            edges.push((hub, w));
        }
    }
    // Random hub→node edges: pure skew (degree). Each hub draws from its OWN
    // disjoint block of node ids, so no random node is shared between hubs and no
    // unintended triangle appears — the closed-form count stays exact.
    let lo = h + b;
    let block = ((n - lo) / h.max(1)).max(1);
    for hub in 0..h {
        let base = lo + hub * block;
        for _ in 0..d.min(block) {
            edges.push((hub, base + rng(block)));
        }
    }

    let c2 = |x: u64| x * (x - 1) / 2;
    let c3 = |x: u64| x * (x - 1) * (x - 2) / 6;
    let expected = b as u64 * c2(h as u64) + c3(h as u64);

    let binary = binary_plan_intermediate(&edges);
    println!(
        "graph: {} hubs, {} bridges, {}×{} random → {} edges",
        h,
        b,
        h,
        d,
        edges.len()
    );
    println!(
        "binary-join intermediate Σd² = {} two-paths  (~{:.1} GB if materialized @16B)",
        binary,
        binary as f64 * 16.0 / 1e9
    );
    println!(
        "WCOJ working set ~ O(m) = ~{:.1} MB (CSR + edges)",
        edges.len() as f64 * 12.0 / 1e6
    );

    let t = Instant::now();
    let tri = count_triangles(&edges).expect("gpu triangles");
    let dt = t.elapsed();
    assert_eq!(tri, expected, "triangle count mismatch");
    println!(
        "WCOJ triangles: {} in {:?}  ({:.1}× smaller working set than the binary plan)",
        tri,
        dt,
        binary as f64 / edges.len() as f64,
    );
    println!(
        "OK — {} triangles, bit-exact, no OOM where the binary plan needs {} tuples.",
        tri, binary
    );
}
