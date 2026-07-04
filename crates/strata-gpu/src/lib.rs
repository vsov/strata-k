//! GPU engine — spec Phase 1, режим A (Bool + Trop over CUDA).
//!
//! Built only with the `cuda` feature (needs an NVIDIA GPU + CUDA toolkit); the
//! kernels are launched from Rust via [`cudarc`] with runtime `nvrtc`
//! compilation. Without the feature the crate compiles to a stub so the rest of
//! the workspace still builds on a machine with no CUDA.
//!
//! Correctness is not assumed: every GPU result must be bit-identical to the CPU
//! reference interpreter (`strata-eval`) — invariant I5. Phase 1 grows this in
//! slices; the first is a Bool transitive closure whose join runs on the GPU.

/// Error from the GPU engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuError {
    /// The crate was built without the `cuda` feature — no GPU code is present.
    NotBuilt,
    /// A CUDA driver / nvrtc error, stringified.
    Cuda(String),
}

impl std::fmt::Display for GpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuError::NotBuilt => write!(
                f,
                "strata-gpu was built without the `cuda` feature (no GPU engine compiled in)"
            ),
            GpuError::Cuda(e) => write!(f, "CUDA error: {e}"),
        }
    }
}

impl std::error::Error for GpuError {}

/// Query planning for WCOJ — hypertree decomposition + cost-based optimizer.
/// Pure CPU logic (produces the plan the WCOJ kernels execute); always built.
pub mod plan;

/// Tensor-contraction query plan (Phase 6): variable-elimination order + a
/// reference sum-product contraction. Pure CPU; always built.
pub mod contraction;

/// Multi-GPU groundwork (Phase 6, spec §7): radix hash-partition + HCube
/// interfaces, with a CPU reference proving union == whole. Always built.
pub mod partition;

/// ASP grounding-simplification (spec §5.2): CPU reference always built; the
/// device kernel is gated behind `cuda`. Bit-identical either way.
pub mod asp;

#[cfg(feature = "cuda")]
mod sort;
#[cfg(feature = "cuda")]
mod tc;
#[cfg(feature = "cuda")]
mod trop;
#[cfg(feature = "cuda")]
mod wcoj;

/// Compute the transitive closure of a Bool binary relation on the GPU.
///
/// `edges` is the EDB (`edge(x, y)` facts); the result is every `path(x, y)`
/// derivable by `path(X,Y):-edge(X,Y). path(X,Z):-edge(X,Y),path(Y,Z).`,
/// returned sorted and deduplicated. Without the `cuda` feature this returns
/// [`GpuError::NotBuilt`].
#[cfg(feature = "cuda")]
pub fn transitive_closure_bool(edges: &[(u32, u32)]) -> Result<Vec<(u32, u32)>, GpuError> {
    tc::transitive_closure_bool(edges)
}

/// Stub when the `cuda` feature is off — keeps the workspace building CUDA-free.
#[cfg(not(feature = "cuda"))]
pub fn transitive_closure_bool(_edges: &[(u32, u32)]) -> Result<Vec<(u32, u32)>, GpuError> {
    Err(GpuError::NotBuilt)
}

/// All-pairs shortest paths over the Trop (min-plus) semiring, on the GPU.
///
/// `edges` are `edge(x, y)` facts each with an `i64` weight; the result is every
/// reachable `(x, z)` with the least total path weight, sorted by `(x, z)`.
/// Requires no negative-weight cycle. Without the `cuda` feature this returns
/// [`GpuError::NotBuilt`].
#[cfg(feature = "cuda")]
pub fn shortest_paths_trop(edges: &[(u32, u32, i64)]) -> Result<Vec<(u32, u32, i64)>, GpuError> {
    trop::shortest_paths_trop(edges)
}

/// Stub when the `cuda` feature is off.
#[cfg(not(feature = "cuda"))]
pub fn shortest_paths_trop(_edges: &[(u32, u32, i64)]) -> Result<Vec<(u32, u32, i64)>, GpuError> {
    Err(GpuError::NotBuilt)
}

/// Count the triangles of the undirected graph on `edges` by a worst-case
/// optimal (leapfrog) join on the GPU — bounded by the AGM bound, so it never
/// materializes the two-path intermediate a binary plan would. [Phase 2]
/// Without the `cuda` feature this returns [`GpuError::NotBuilt`].
#[cfg(feature = "cuda")]
pub fn count_triangles(edges: &[(u32, u32)]) -> Result<u64, GpuError> {
    wcoj::count_triangles(edges)
}

/// Stub when the `cuda` feature is off.
#[cfg(not(feature = "cuda"))]
pub fn count_triangles(_edges: &[(u32, u32)]) -> Result<u64, GpuError> {
    Err(GpuError::NotBuilt)
}

/// Count the 4-cliques (K4) of the undirected graph on `edges` by GPU WCOJ (a
/// 3-way nested leapfrog). Without the `cuda` feature, [`GpuError::NotBuilt`].
#[cfg(feature = "cuda")]
pub fn count_4cliques(edges: &[(u32, u32)]) -> Result<u64, GpuError> {
    wcoj::count_4cliques(edges)
}

/// Stub when the `cuda` feature is off.
#[cfg(not(feature = "cuda"))]
pub fn count_4cliques(_edges: &[(u32, u32)]) -> Result<u64, GpuError> {
    Err(GpuError::NotBuilt)
}

/// Device-side term interning [Phase 3]: deduplicate the `u64` term keys on the
/// GPU and return each input's interned id (its index in the sorted distinct
/// table). Equal keys get equal ids; the id space is dense `0..distinct`. The
/// GPU counterpart to the host interner, for when profiling shows interning
/// dominates. Without the `cuda` feature, [`GpuError::NotBuilt`].
#[cfg(feature = "cuda")]
pub fn intern_terms(keys: &[u64]) -> Result<Vec<u32>, GpuError> {
    sort::intern_terms_dev(keys)
}

/// Stub when the `cuda` feature is off.
#[cfg(not(feature = "cuda"))]
pub fn intern_terms(_keys: &[u64]) -> Result<Vec<u32>, GpuError> {
    Err(GpuError::NotBuilt)
}

/// Size of the binary-plan intermediate `edge ⋈ edge` (Σ_v d(v)²) — the
/// allocation a two-path materialization would need and WCOJ avoids.
#[cfg(feature = "cuda")]
pub fn binary_plan_intermediate(edges: &[(u32, u32)]) -> u64 {
    wcoj::binary_plan_intermediate(edges)
}

/// Stub when the `cuda` feature is off.
#[cfg(not(feature = "cuda"))]
pub fn binary_plan_intermediate(_edges: &[(u32, u32)]) -> u64 {
    0
}
