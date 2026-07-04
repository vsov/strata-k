//! GPU sort / unique / min-by-key primitives on `u64` keys. [Phase 1, slice 5-7]
//!
//! Moves relation set-maintenance off the host. A binary relation's `(src, dst)`
//! pair encodes into one `u64` key (`(src as u64) << 32 | dst`), so sorting the
//! keys sorts the relation into the canonical `(src, dst)` order the join
//! kernels rely on. Deduplicating unique keys deduplicates a Bool relation;
//! keeping the minimum weight per key aggregates a Trop relation (its `⊕`).
//!
//! - **sort (keys)**: an LSD **radix sort**, one bit per pass (64 passes),
//!   each a stable split — compute the bit, exclusive-scan the zeros to place
//!   them, then scatter (ping-pong buffers). O(n) per pass, no padding.
//! - **sort (key+weight)**: a bitonic network ordered by (key asc, weight asc)
//!   for the Trop min path (still O(n log² n)).
//! - **unique / min-by-key**: flag first-of-run, exclusive-scan the flags to
//!   output positions (two-level — block scans + a host scan of block sums), and
//!   scatter the surviving rows.
//!
//! Still host-driven at the boundaries; on-device join→sort fusion is a later
//! slice.

use std::sync::{Arc, OnceLock};

use cudarc::driver::{CudaDevice, CudaSlice, LaunchAsync, LaunchConfig};
use cudarc::nvrtc::compile_ptx;

use crate::GpuError;

const BLOCK: u32 = 256;

const KERNELS: &str = r#"
// notbit[i] = 1 - bit b of key[i]  (the "goes-left" flag of a radix split).
extern "C" __global__ void compute_notbit(const unsigned long long* keys, int b, int n,
                                           unsigned int* notbit) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n) return;
    notbit[i] = 1u - (unsigned int)((keys[i] >> b) & 1ULL);
}

// Stable radix split: zeros to their scanned rank, ones after all zeros.
extern "C" __global__ void scatter_split(const unsigned long long* keys_in, int b,
                                         const unsigned int* scan_zeros, int n,
                                         unsigned int total_zeros, unsigned long long* keys_out) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n) return;
    unsigned int bit = (unsigned int)((keys_in[i] >> b) & 1ULL);
    unsigned int dest = bit ? (total_zeros + ((unsigned int)i - scan_zeros[i])) : scan_zeros[i];
    keys_out[dest] = keys_in[i];
}

// Key-value bitonic stage, ordered lexicographically by (key asc, weight asc).
extern "C" __global__ void bitonic_step_kv(unsigned long long* keys, long long* w,
                                           int n, int k, int j) {
    unsigned int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= (unsigned int)n) return;
    unsigned int l = i ^ (unsigned int)j;
    if (l > i) {
        bool ascending = ((i & (unsigned int)k) == 0);
        unsigned long long ki = keys[i], kl = keys[l];
        long long wi = w[i], wl = w[l];
        bool greater = (ki > kl) || (ki == kl && wi > wl);
        if (greater == ascending) {
            keys[i] = kl; keys[l] = ki;
            w[i] = wl; w[l] = wi;
        }
    }
}

// flags[i] = 1 iff a[i] starts a new run (a is sorted by key).
extern "C" __global__ void flag_unique(const unsigned long long* a, int n, unsigned int* flags) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n) return;
    flags[i] = (i == 0 || a[i] != a[i - 1]) ? 1u : 0u;
}

// Per-block exclusive scan (Hillis-Steele); blocksums[b] = block's total.
extern "C" __global__ void scan_block(const unsigned int* in, unsigned int* out,
                                      unsigned int* blocksums, int n) {
    extern __shared__ unsigned int tmp[];
    int tid = threadIdx.x;
    int gid = blockIdx.x * blockDim.x + tid;
    unsigned int v = (gid < n) ? in[gid] : 0u;
    tmp[tid] = v;
    __syncthreads();
    for (int off = 1; off < blockDim.x; off <<= 1) {
        unsigned int t = (tid >= off) ? tmp[tid - off] : 0u;
        __syncthreads();
        tmp[tid] += t;
        __syncthreads();
    }
    if (gid < n) out[gid] = tmp[tid] - v;
    if (tid == blockDim.x - 1) blocksums[blockIdx.x] = tmp[tid];
}

extern "C" __global__ void scan_add(unsigned int* out, const unsigned int* blockoff, int n) {
    int gid = blockIdx.x * blockDim.x + threadIdx.x;
    if (gid < n) out[gid] += blockoff[blockIdx.x];
}

extern "C" __global__ void scatter_unique(const unsigned long long* a, const unsigned int* flags,
                                          const unsigned int* pos, int n, unsigned long long* out) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n && flags[i]) out[pos[i]] = a[i];
}

extern "C" __global__ void scatter_unique_kv(const unsigned long long* keys, const long long* w,
                                             const unsigned int* flags, const unsigned int* pos,
                                             int n, unsigned long long* okeys, long long* ow) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n && flags[i]) { okeys[pos[i]] = keys[i]; ow[pos[i]] = w[i]; }
}

// --- device-resident Bool transitive closure -------------------------------
// delta is stored as sorted (src,dst) keys; its src is the high 32 bits, so the
// join binary-searches that. Bounds of the run where (dkey>>32) == key:
__device__ int bj_lb(const unsigned long long* d, int n, unsigned int key) {
    int lo = 0, hi = n;
    while (lo < hi) { int m = (lo + hi) >> 1; if ((unsigned int)(d[m] >> 32) < key) lo = m + 1; else hi = m; }
    return lo;
}
__device__ int bj_ub(const unsigned long long* d, int n, unsigned int key) {
    int lo = 0, hi = n;
    while (lo < hi) { int m = (lo + hi) >> 1; if ((unsigned int)(d[m] >> 32) <= key) lo = m + 1; else hi = m; }
    return lo;
}
extern "C" __global__ void bj_count(const unsigned int* edst, int ne,
                                    const unsigned long long* dkeys, int nd, unsigned int* counts) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= ne) return;
    unsigned int key = edst[i];
    counts[i] = (unsigned int)(bj_ub(dkeys, nd, key) - bj_lb(dkeys, nd, key));
}
// emit (E.src, Δ.dst) = (esrc[i] << 32) | (dkey & 0xffffffff)
extern "C" __global__ void bj_emit(const unsigned int* esrc, const unsigned int* edst, int ne,
                                   const unsigned long long* dkeys, int nd,
                                   const unsigned int* offsets, unsigned long long* out) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= ne) return;
    unsigned int key = edst[i];
    int lo = bj_lb(dkeys, nd, key), hi = bj_ub(dkeys, nd, key);
    unsigned int off = offsets[i];
    for (int k = lo; k < hi; k++) {
        out[off + (k - lo)] = ((unsigned long long)esrc[i] << 32) | (dkeys[k] & 0xffffffffULL);
    }
}
// flags[i] = 1 iff cand[i] is NOT already present in the sorted path.
extern "C" __global__ void diff_flag(const unsigned long long* cand, int nc,
                                     const unsigned long long* path, int np, unsigned int* flags) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= nc) return;
    unsigned long long key = cand[i];
    int lo = 0, hi = np;
    while (lo < hi) { int m = (lo + hi) >> 1; if (path[m] < key) lo = m + 1; else hi = m; }
    flags[i] = (lo < np && path[lo] == key) ? 0u : 1u;
}
// dst[off + i] = src[i]
extern "C" __global__ void copy_u64(const unsigned long long* src, int n,
                                    unsigned long long* dst, int off) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n) dst[off + i] = src[i];
}
extern "C" __global__ void copy_i64(const long long* src, int n, long long* dst, int off) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n) dst[off + i] = src[i];
}
extern "C" __global__ void fill_u64(unsigned long long* buf, int off, int n, unsigned long long v) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n) buf[off + i] = v;
}
extern "C" __global__ void fill_i64(long long* buf, int off, int n, long long v) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n) buf[off + i] = v;
}

// --- device-resident Trop (min-plus) join / improvement --------------------
// Weighted emit: new key = (E.src<<32)|Δ.dst, new weight = E.w + Δ.w.
extern "C" __global__ void bjt_emit(const unsigned int* esrc, const unsigned int* edst,
                                    const long long* ew, int ne,
                                    const unsigned long long* dkeys, const long long* dw, int nd,
                                    const unsigned int* offsets,
                                    unsigned long long* okeys, long long* ow) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= ne) return;
    unsigned int key = edst[i];
    int lo = bj_lb(dkeys, nd, key), hi = bj_ub(dkeys, nd, key);
    unsigned int off = offsets[i];
    for (int k = lo; k < hi; k++) {
        okeys[off + (k - lo)] = ((unsigned long long)esrc[i] << 32) | (dkeys[k] & 0xffffffffULL);
        ow[off + (k - lo)] = ew[i] + dw[k];
    }
}
// flags[i] = 1 iff cand key is new to path, or improves its weight.
extern "C" __global__ void diff_improve(const unsigned long long* candk, const long long* candw,
                                        int nc, const unsigned long long* pathk,
                                        const long long* pathw, int np, unsigned int* flags) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= nc) return;
    unsigned long long key = candk[i];
    int lo = 0, hi = np;
    while (lo < hi) { int m = (lo + hi) >> 1; if (pathk[m] < key) lo = m + 1; else hi = m; }
    bool present = (lo < np && pathk[lo] == key);
    flags[i] = (!present || candw[i] < pathw[lo]) ? 1u : 0u;
}

// Device-side term interning: map each key to its index in the sorted distinct
// `table` (its interned id) by binary search. [Phase 3: device interning]
extern "C" __global__ void intern_map(const unsigned long long* keys, int n,
                                      const unsigned long long* table, int m, unsigned int* ids) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n) return;
    unsigned long long key = keys[i];
    int lo = 0, hi = m;
    while (lo < hi) { int mid = (lo + hi) >> 1; if (table[mid] < key) lo = mid + 1; else hi = mid; }
    ids[i] = (unsigned int)lo; // table[lo] == key (always present)
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
        dev.load_ptx(
            ptx,
            "sort",
            &[
                "compute_notbit",
                "scatter_split",
                "bitonic_step_kv",
                "flag_unique",
                "scan_block",
                "scan_add",
                "scatter_unique",
                "scatter_unique_kv",
                "bj_count",
                "bj_emit",
                "diff_flag",
                "copy_u64",
                "copy_i64",
                "fill_u64",
                "fill_i64",
                "bjt_emit",
                "diff_improve",
                "intern_map",
            ],
        )
        .map_err(cuda)?;
        Ok(dev)
    })
    .clone()
}

/// Shared device with all kernels loaded (used by the device-resident engines).
pub(crate) fn shared_device() -> Result<Arc<CudaDevice>, GpuError> {
    device()
}

fn next_pow2(n: usize) -> usize {
    let mut p = 1usize;
    while p < n {
        p <<= 1;
    }
    p
}

fn blocks_for(n: usize) -> u32 {
    (n as u32).div_ceil(BLOCK)
}

fn cfg(blocks: u32, smem: u32) -> LaunchConfig {
    LaunchConfig {
        grid_dim: (blocks.max(1), 1, 1),
        block_dim: (BLOCK, 1, 1),
        shared_mem_bytes: smem,
    }
}

/// Exclusive scan of a device flag array → (positions, total set).
pub(crate) fn scan_flags(
    dev: &Arc<CudaDevice>,
    flags: &CudaSlice<u32>,
    n: usize,
) -> Result<(CudaSlice<u32>, usize), GpuError> {
    let nb = blocks_for(n);
    let mut pos = dev.alloc_zeros::<u32>(n).map_err(cuda)?;
    let mut blocksums = dev.alloc_zeros::<u32>(nb as usize).map_err(cuda)?;
    let scan_fn = dev.get_func("sort", "scan_block").unwrap();
    unsafe {
        scan_fn
            .launch(
                cfg(nb, BLOCK * 4),
                (flags, &mut pos, &mut blocksums, n as i32),
            )
            .map_err(cuda)?;
    }
    let bs_h = dev.dtoh_sync_copy(&blocksums).map_err(cuda)?;
    let mut blockoff_h = vec![0u32; nb as usize];
    let mut acc = 0u32;
    for (o, &s) in blockoff_h.iter_mut().zip(bs_h.iter()) {
        *o = acc;
        acc += s;
    }
    let total = acc as usize;
    let blockoff = dev.htod_copy(blockoff_h).map_err(cuda)?;
    let add_fn = dev.get_func("sort", "scan_add").unwrap();
    unsafe {
        add_fn
            .launch(cfg(nb, 0), (&mut pos, &blockoff, n as i32))
            .map_err(cuda)?;
    }
    Ok((pos, total))
}

pub(crate) fn flag_first_of_run(
    dev: &Arc<CudaDevice>,
    keys: &CudaSlice<u64>,
    n: usize,
) -> Result<CudaSlice<u32>, GpuError> {
    let mut flags = dev.alloc_zeros::<u32>(n).map_err(cuda)?;
    let flag_fn = dev.get_func("sort", "flag_unique").unwrap();
    unsafe {
        flag_fn
            .launch(
                LaunchConfig::for_num_elems(n as u32),
                (keys, n as i32, &mut flags),
            )
            .map_err(cuda)?;
    }
    Ok(flags)
}

/// LSD radix sort of `a` (length `n`) ascending, one bit per pass. Ping-pongs
/// between two buffers; 64 passes leave the result back in the first buffer.
pub(crate) fn radix_sort_u64(
    dev: &Arc<CudaDevice>,
    mut a: CudaSlice<u64>,
    n: usize,
) -> Result<CudaSlice<u64>, GpuError> {
    let launch = LaunchConfig::for_num_elems(n as u32);
    let mut b = dev.alloc_zeros::<u64>(n).map_err(cuda)?;
    let mut notbit = dev.alloc_zeros::<u32>(n).map_err(cuda)?;
    for bit in 0..64i32 {
        let nb_fn = dev.get_func("sort", "compute_notbit").unwrap();
        unsafe {
            nb_fn
                .launch(launch, (&a, bit, n as i32, &mut notbit))
                .map_err(cuda)?;
        }
        let (scan_zeros, total_zeros) = scan_flags(dev, &notbit, n)?;
        let sc_fn = dev.get_func("sort", "scatter_split").unwrap();
        unsafe {
            sc_fn
                .launch(
                    launch,
                    (&a, bit, &scan_zeros, n as i32, total_zeros as u32, &mut b),
                )
                .map_err(cuda)?;
        }
        std::mem::swap(&mut a, &mut b);
    }
    Ok(a) // 64 (even) swaps → sorted result is in `a`
}

/// Key-value bitonic sort by (key asc, weight asc) over power-of-two arrays.
fn bitonic_kv(
    dev: &Arc<CudaDevice>,
    keys: &mut CudaSlice<u64>,
    w: &mut CudaSlice<i64>,
    pad: usize,
) -> Result<(), GpuError> {
    let launch = LaunchConfig::for_num_elems(pad as u32);
    let mut k = 2usize;
    while k <= pad {
        let mut j = k >> 1;
        while j > 0 {
            let step = dev.get_func("sort", "bitonic_step_kv").unwrap();
            unsafe {
                step.launch(
                    launch,
                    (&mut *keys, &mut *w, pad as i32, k as i32, j as i32),
                )
                .map_err(cuda)?;
            }
            j >>= 1;
        }
        k <<= 1;
    }
    Ok(())
}

/// Sort `keys` ascending on the GPU (duplicates kept).
#[allow(dead_code)]
pub(crate) fn gpu_sort_u64(keys: &[u64]) -> Result<Vec<u64>, GpuError> {
    let n = keys.len();
    if n <= 1 {
        return Ok(keys.to_vec());
    }
    let dev = device()?;
    let a = dev.htod_copy(keys.to_vec()).map_err(cuda)?;
    let a = radix_sort_u64(&dev, a, n)?;
    dev.dtoh_sync_copy(&a).map_err(cuda)
}

/// Sort + remove duplicates, entirely on the GPU (host in/out; unit-tested).
#[allow(dead_code)]
pub(crate) fn gpu_sort_unique_u64(keys: &[u64]) -> Result<Vec<u64>, GpuError> {
    let n = keys.len();
    if n <= 1 {
        return Ok(keys.to_vec());
    }
    let dev = device()?;
    let a = dev.htod_copy(keys.to_vec()).map_err(cuda)?;
    let a = radix_sort_u64(&dev, a, n)?;

    let flags = flag_first_of_run(&dev, &a, n)?;
    let (pos, total) = scan_flags(&dev, &flags, n)?;

    let mut out = dev.alloc_zeros::<u64>(total.max(1)).map_err(cuda)?;
    let scatter_fn = dev.get_func("sort", "scatter_unique").unwrap();
    unsafe {
        scatter_fn
            .launch(
                LaunchConfig::for_num_elems(n as u32),
                (&a, &flags, &pos, n as i32, &mut out),
            )
            .map_err(cuda)?;
    }
    let mut unique = dev.dtoh_sync_copy(&out).map_err(cuda)?;
    unique.truncate(total);
    Ok(unique)
}

/// Device-in/out sort + dedup: returns (unique keys buffer on device, count).
/// The building block for the fully device-resident closure loop.
pub(crate) fn sort_unique_dev(
    dev: &Arc<CudaDevice>,
    a: CudaSlice<u64>,
    n: usize,
) -> Result<(CudaSlice<u64>, usize), GpuError> {
    if n <= 1 {
        return Ok((a, n));
    }
    let a = radix_sort_u64(dev, a, n)?;
    let flags = flag_first_of_run(dev, &a, n)?;
    let (pos, total) = scan_flags(dev, &flags, n)?;
    let mut out = dev.alloc_zeros::<u64>(total.max(1)).map_err(cuda)?;
    let scatter_fn = dev.get_func("sort", "scatter_unique").unwrap();
    unsafe {
        scatter_fn
            .launch(
                LaunchConfig::for_num_elems(n as u32),
                (&a, &flags, &pos, n as i32, &mut out),
            )
            .map_err(cuda)?;
    }
    Ok((out, total))
}

/// Device-in/out min-by-key: one row per key with the minimum weight (Trop `⊕`).
/// Returns (keys, weights, count), all on device. The building block for the
/// device-resident shortest-paths loop.
pub(crate) fn sort_min_dev(
    dev: &Arc<CudaDevice>,
    keys: CudaSlice<u64>,
    w: CudaSlice<i64>,
    n: usize,
) -> Result<(CudaSlice<u64>, CudaSlice<i64>, usize), GpuError> {
    if n <= 1 {
        return Ok((keys, w, n));
    }
    // Pad to a power of two with maximal sentinels for the bitonic kv-sort.
    let pad = next_pow2(n);
    let mut pk = dev.alloc_zeros::<u64>(pad).map_err(cuda)?;
    let mut pw = dev.alloc_zeros::<i64>(pad).map_err(cuda)?;
    let cfgn = LaunchConfig::for_num_elems(n as u32);
    let cu = dev.get_func("sort", "copy_u64").unwrap();
    let ci = dev.get_func("sort", "copy_i64").unwrap();
    unsafe {
        cu.launch(cfgn, (&keys, n as i32, &mut pk, 0i32))
            .map_err(cuda)?;
        ci.launch(cfgn, (&w, n as i32, &mut pw, 0i32))
            .map_err(cuda)?;
    }
    if pad > n {
        let tail = (pad - n) as u32;
        let cfgt = LaunchConfig::for_num_elems(tail);
        let fu = dev.get_func("sort", "fill_u64").unwrap();
        let fi = dev.get_func("sort", "fill_i64").unwrap();
        unsafe {
            fu.launch(cfgt, (&mut pk, n as i32, tail as i32, u64::MAX))
                .map_err(cuda)?;
            fi.launch(cfgt, (&mut pw, n as i32, tail as i32, i64::MAX))
                .map_err(cuda)?;
        }
    }
    bitonic_kv(dev, &mut pk, &mut pw, pad)?;
    let flags = flag_first_of_run(dev, &pk, n)?;
    let (pos, total) = scan_flags(dev, &flags, n)?;
    let mut okeys = dev.alloc_zeros::<u64>(total.max(1)).map_err(cuda)?;
    let mut ow = dev.alloc_zeros::<i64>(total.max(1)).map_err(cuda)?;
    let scv = dev.get_func("sort", "scatter_unique_kv").unwrap();
    unsafe {
        scv.launch(
            LaunchConfig::for_num_elems(n as u32),
            (&pk, &pw, &flags, &pos, n as i32, &mut okeys, &mut ow),
        )
        .map_err(cuda)?;
    }
    Ok((okeys, ow, total))
}

/// Sort + dedup a Bool relation's `(src, dst)` pairs on the GPU (host in/out;
/// superseded in the closure loop by the device-resident path, kept + tested).
#[allow(dead_code)]
pub(crate) fn sort_unique_pairs(pairs: &[(u32, u32)]) -> Result<Vec<(u32, u32)>, GpuError> {
    if pairs.is_empty() {
        return Ok(Vec::new());
    }
    let keys: Vec<u64> = pairs
        .iter()
        .map(|&(s, d)| ((s as u64) << 32) | (d as u64))
        .collect();
    let u = gpu_sort_unique_u64(&keys)?;
    Ok(u.into_iter()
        .map(|k| ((k >> 32) as u32, (k & 0xffff_ffff) as u32))
        .collect())
}

/// Device-side term interning: deduplicate `keys` on the GPU and return each
/// input's interned id (its index in the sorted distinct table). Reuses the
/// GPU sort+unique path — the "device-side interning by profile" of Phase 3.
pub(crate) fn intern_terms_dev(keys: &[u64]) -> Result<Vec<u32>, GpuError> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let table = gpu_sort_unique_u64(keys)?; // sorted distinct = the term table
    let dev = device()?;
    let d_keys = dev.htod_copy(keys.to_vec()).map_err(cuda)?;
    let d_table = dev.htod_copy(table.clone()).map_err(cuda)?;
    let mut ids = dev.alloc_zeros::<u32>(keys.len()).map_err(cuda)?;
    let f = dev.get_func("sort", "intern_map").unwrap();
    unsafe {
        f.launch(
            LaunchConfig::for_num_elems(keys.len() as u32),
            (
                &d_keys,
                keys.len() as i32,
                &d_table,
                table.len() as i32,
                &mut ids,
            ),
        )
        .map_err(cuda)?;
    }
    dev.dtoh_sync_copy(&ids).map_err(cuda)
}

/// Aggregate a Trop relation on the GPU: one row per `(src, dst)` key carrying
/// the minimum weight (the Trop `⊕`). Host in/out; superseded in the loop by the
/// device-resident [`sort_min_dev`], kept + unit-tested.
#[allow(dead_code)]
pub(crate) fn sort_min_pairs(pairs: &[(u32, u32, i64)]) -> Result<Vec<(u32, u32, i64)>, GpuError> {
    let n = pairs.len();
    if n <= 1 {
        return Ok(pairs.to_vec());
    }
    let dev = device()?;
    let pad = next_pow2(n);
    let mut kh: Vec<u64> = pairs
        .iter()
        .map(|&(s, d, _)| ((s as u64) << 32) | (d as u64))
        .collect();
    let mut wh: Vec<i64> = pairs.iter().map(|&(_, _, w)| w).collect();
    kh.resize(pad, u64::MAX);
    wh.resize(pad, i64::MAX);
    let mut keys = dev.htod_copy(kh).map_err(cuda)?;
    let mut w = dev.htod_copy(wh).map_err(cuda)?;

    bitonic_kv(&dev, &mut keys, &mut w, pad)?;
    let flags = flag_first_of_run(&dev, &keys, n)?;
    let (pos, total) = scan_flags(&dev, &flags, n)?;

    let mut okeys = dev.alloc_zeros::<u64>(total.max(1)).map_err(cuda)?;
    let mut ow = dev.alloc_zeros::<i64>(total.max(1)).map_err(cuda)?;
    let scatter_fn = dev.get_func("sort", "scatter_unique_kv").unwrap();
    unsafe {
        scatter_fn
            .launch(
                LaunchConfig::for_num_elems(n as u32),
                (&keys, &w, &flags, &pos, n as i32, &mut okeys, &mut ow),
            )
            .map_err(cuda)?;
    }
    let mut okeys_h = dev.dtoh_sync_copy(&okeys).map_err(cuda)?;
    let mut ow_h = dev.dtoh_sync_copy(&ow).map_err(cuda)?;
    okeys_h.truncate(total);
    ow_h.truncate(total);
    Ok(okeys_h
        .into_iter()
        .zip(ow_h)
        .map(|(k, weight)| ((k >> 32) as u32, (k & 0xffff_ffff) as u32, weight))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{gpu_sort_u64, gpu_sort_unique_u64, sort_min_pairs, sort_unique_pairs};
    use std::collections::BTreeMap;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 11
        }
    }

    #[test]
    fn sorts_like_host() {
        let mut rng = Rng(0xa11ce);
        for &n in &[0usize, 1, 2, 3, 5, 7, 8, 100, 1000, 4097] {
            // Full 64-bit range to exercise every radix pass.
            let keys: Vec<u64> = (0..n).map(|_| rng.next()).collect();
            let got = gpu_sort_u64(&keys).expect("gpu sort");
            let mut want = keys.clone();
            want.sort_unstable();
            assert_eq!(got, want, "n={n}");
        }
    }

    #[test]
    fn unique_like_host() {
        let mut rng = Rng(0xbeef);
        for &n in &[0usize, 1, 2, 4, 9, 255, 256, 257, 1000, 5000] {
            let keys: Vec<u64> = (0..n).map(|_| rng.next() % 40).collect();
            let got = gpu_sort_unique_u64(&keys).expect("gpu unique");
            let mut want = keys.clone();
            want.sort_unstable();
            want.dedup();
            assert_eq!(got, want, "n={n}");
        }
    }

    #[test]
    fn pairs_roundtrip() {
        let mut rng = Rng(0xf00d);
        for _ in 0..20 {
            let m = (rng.next() % 500) as usize;
            let pairs: Vec<(u32, u32)> = (0..m)
                .map(|_| ((rng.next() % 30) as u32, (rng.next() % 30) as u32))
                .collect();
            let got = sort_unique_pairs(&pairs).expect("pairs");
            let mut want = pairs.clone();
            want.sort_unstable();
            want.dedup();
            assert_eq!(got, want);
        }
    }

    #[test]
    fn min_by_key_like_host() {
        let mut rng = Rng(0x5eed);
        for _ in 0..30 {
            let m = (rng.next() % 600) as usize;
            let triples: Vec<(u32, u32, i64)> = (0..m)
                .map(|_| {
                    (
                        (rng.next() % 20) as u32,
                        (rng.next() % 20) as u32,
                        (rng.next() % 100) as i64 - 20,
                    )
                })
                .collect();
            let got = sort_min_pairs(&triples).expect("min");
            let mut want: BTreeMap<(u32, u32), i64> = BTreeMap::new();
            for &(s, d, w) in &triples {
                want.entry((s, d))
                    .and_modify(|e| *e = (*e).min(w))
                    .or_insert(w);
            }
            let want: Vec<(u32, u32, i64)> =
                want.into_iter().map(|((s, d), w)| (s, d, w)).collect();
            assert_eq!(got, want);
        }
    }
}
