//! Worst-case-optimal join on the GPU — triangle counting by leapfrog. [Phase 2]
//!
//! A binary join plan for `tri(x,y,z) :- edge(x,y), edge(y,z), edge(x,z)` first
//! materializes the two-paths `edge ⋈ edge` — Σ_v d(v)² tuples — which explodes
//! on skewed (power-law) graphs even when few triangles exist. A **worst-case
//! optimal join** (leapfrog triejoin) instead intersects the participating
//! relations one variable at a time, so its work/memory is bounded by the AGM
//! bound (O(m^{3/2}) for triangles) and it never builds the intermediate.
//!
//! Triangles specialize the leapfrog intersection to adjacency lists. Orient
//! each undirected edge `u<v` and keep, per node, its forward neighbours
//! `adj⁺(v) = { w ∈ N(v) : w > v }` sorted (a two-level trie: node → neighbours).
//! Then for every oriented edge `(u,v)` the triangles `u<v<w` are exactly the
//! **sorted-list intersection** `adj⁺(u) ∩ adj⁺(v)` — one leapfrog per edge, done
//! in parallel on the GPU. Memory is O(m) (the CSR trie), independent of Σ d².

use std::sync::{Arc, OnceLock};

use cudarc::driver::{CudaDevice, LaunchAsync, LaunchConfig};
use cudarc::nvrtc::compile_ptx;

use crate::GpuError;

const KERNELS: &str = r#"
// One thread per oriented edge (u,v): count |adj+(u) ∩ adj+(v)| by a merge
// (leapfrog) of the two sorted forward-neighbour lists; add into a global total.
extern "C" __global__ void tri_count(const unsigned int* eu, const unsigned int* ev, int ne,
                                     const unsigned int* off, const unsigned int* adj,
                                     unsigned long long* total) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= ne) return;
    unsigned int u = eu[i], v = ev[i];
    int a = off[u], ae = off[u + 1];
    int b = off[v], be = off[v + 1];
    unsigned long long c = 0;
    while (a < ae && b < be) {
        unsigned int x = adj[a], y = adj[b];
        if (x < y) a++;
        else if (x > y) b++;
        else { c++; a++; b++; }
    }
    if (c) atomicAdd(total, c);
}

// 4-clique count: for each oriented edge (u,v), walk the common neighbours
// w1 ∈ adj+(u) ∩ adj+(v) (each a triangle apex), and for each w1 count the
// w2 that close a 4-clique via a 3-way leapfrog adj+(u) ∩ adj+(v) ∩ adj+(w1)
// past w1. Each K4 {u<v<w1<w2} is counted once. Streaming — no set stored.
extern "C" __global__ void tetra_count(const unsigned int* eu, const unsigned int* ev, int ne,
                                       const unsigned int* off, const unsigned int* adj,
                                       unsigned long long* total) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= ne) return;
    unsigned int u = eu[i], v = ev[i];
    int a = off[u], ae = off[u + 1];
    int b = off[v], be = off[v + 1];
    unsigned long long c = 0;
    while (a < ae && b < be) {
        unsigned int x = adj[a], y = adj[b];
        if (x < y) { a++; continue; }
        if (x > y) { b++; continue; }
        unsigned int w1 = x;                          // triangle apex u<v<w1
        int p = a + 1, q = b + 1, r = off[w1], re = off[w1 + 1];
        while (p < ae && q < be && r < re) {          // adj+(u) ∩ adj+(v) ∩ adj+(w1)
            unsigned int xp = adj[p], xq = adj[q], xr = adj[r];
            unsigned int mx = xp;
            if (xq > mx) mx = xq;
            if (xr > mx) mx = xr;
            if (xp < mx) p++;
            else if (xq < mx) q++;
            else if (xr < mx) r++;
            else { c++; p++; q++; r++; }
        }
        a++; b++;
    }
    if (c) atomicAdd(total, c);
}
"#;

fn cuda<E: std::fmt::Display>(e: E) -> GpuError {
    GpuError::Cuda(e.to_string())
}

fn device() -> Result<Arc<CudaDevice>, GpuError> {
    static DEV: OnceLock<Result<Arc<CudaDevice>, GpuError>> = OnceLock::new();
    DEV.get_or_init(|| {
        let dev = CudaDevice::new(0).map_err(cuda)?;
        let ptx = compile_ptx(KERNELS).map_err(cuda)?;
        dev.load_ptx(ptx, "wcoj", &["tri_count", "tetra_count"])
            .map_err(cuda)?;
        Ok(dev)
    })
    .clone()
}

/// The oriented forward-neighbour trie: `(nnodes, off, adj, eu, ev)`.
/// `adj[off[u]..off[u+1]]` are `u`'s sorted forward neighbours; `(eu[i], ev[i])`
/// is the i-th oriented edge.
fn build_trie(edges: &[(u32, u32)]) -> (usize, Vec<u32>, Vec<u32>, Vec<u32>, Vec<u32>) {
    // Orient u<v, drop self-loops, dedup → a simple undirected graph.
    let mut oriented: Vec<(u32, u32)> = edges
        .iter()
        .filter_map(|&(a, b)| match a.cmp(&b) {
            std::cmp::Ordering::Less => Some((a, b)),
            std::cmp::Ordering::Greater => Some((b, a)),
            std::cmp::Ordering::Equal => None,
        })
        .collect();
    oriented.sort_unstable();
    oriented.dedup();

    let nnodes = oriented
        .iter()
        .flat_map(|&(a, b)| [a, b])
        .max()
        .map_or(0, |m| m as usize + 1);

    // CSR by source; oriented is sorted by (u,v) so each u's neighbours are
    // already contiguous and ascending.
    let mut off = vec![0u32; nnodes + 1];
    for &(u, _) in &oriented {
        off[u as usize + 1] += 1;
    }
    for i in 0..nnodes {
        off[i + 1] += off[i];
    }
    let adj: Vec<u32> = oriented.iter().map(|&(_, v)| v).collect();
    let eu: Vec<u32> = oriented.iter().map(|&(u, _)| u).collect();
    let ev: Vec<u32> = oriented.iter().map(|&(_, v)| v).collect();
    (nnodes, off, adj, eu, ev)
}

/// Count the triangles of the undirected graph on `edges` by GPU WCOJ.
/// Duplicate edges, both directions, and self-loops are handled; each triangle
/// is counted exactly once. Without the `cuda` feature, [`GpuError::NotBuilt`].
pub fn count_triangles(edges: &[(u32, u32)]) -> Result<u64, GpuError> {
    let (_nnodes, off, adj, eu, ev) = build_trie(edges);
    let ne = eu.len();
    if ne == 0 {
        return Ok(0);
    }
    let dev = device()?;
    let d_off = dev.htod_copy(off).map_err(cuda)?;
    let d_adj = dev.htod_copy(adj).map_err(cuda)?;
    let d_eu = dev.htod_copy(eu).map_err(cuda)?;
    let d_ev = dev.htod_copy(ev).map_err(cuda)?;
    let mut total = dev.alloc_zeros::<u64>(1).map_err(cuda)?;

    let f = dev.get_func("wcoj", "tri_count").unwrap();
    unsafe {
        f.launch(
            LaunchConfig::for_num_elems(ne as u32),
            (&d_eu, &d_ev, ne as i32, &d_off, &d_adj, &mut total),
        )
        .map_err(cuda)?;
    }
    Ok(dev.dtoh_sync_copy(&total).map_err(cuda)?[0])
}

/// Count the 4-cliques (K4) of the undirected graph on `edges` by GPU WCOJ
/// (a 3-way nested leapfrog). Each clique counted once; no intermediate stored.
pub fn count_4cliques(edges: &[(u32, u32)]) -> Result<u64, GpuError> {
    let (_nnodes, off, adj, eu, ev) = build_trie(edges);
    let ne = eu.len();
    if ne == 0 {
        return Ok(0);
    }
    let dev = device()?;
    let d_off = dev.htod_copy(off).map_err(cuda)?;
    let d_adj = dev.htod_copy(adj).map_err(cuda)?;
    let d_eu = dev.htod_copy(eu).map_err(cuda)?;
    let d_ev = dev.htod_copy(ev).map_err(cuda)?;
    let mut total = dev.alloc_zeros::<u64>(1).map_err(cuda)?;

    let f = dev.get_func("wcoj", "tetra_count").unwrap();
    unsafe {
        f.launch(
            LaunchConfig::for_num_elems(ne as u32),
            (&d_eu, &d_ev, ne as i32, &d_off, &d_adj, &mut total),
        )
        .map_err(cuda)?;
    }
    Ok(dev.dtoh_sync_copy(&total).map_err(cuda)?[0])
}

/// The size of the binary-plan intermediate `edge ⋈ edge` (Σ_v d(v)² over the
/// oriented graph) — the number a two-path materialization would allocate, which
/// WCOJ avoids. Exposed so the skew benchmark can show the blow-up it dodges.
pub fn binary_plan_intermediate(edges: &[(u32, u32)]) -> u64 {
    let (nnodes, off, _adj, _eu, _ev) = build_trie(edges);
    (0..nnodes)
        .map(|u| {
            let d = (off[u + 1] - off[u]) as u64;
            d * d
        })
        .sum()
}
