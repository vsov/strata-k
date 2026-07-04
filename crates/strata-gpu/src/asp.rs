//! GPU grounding-simplification: the §5.2 "simplify on the GPU before transfer"
//! pass. [Phase 5]
//!
//! Spec §5.1 assigns grounding (a bottom-up fixpoint) to the GPU and CDNL to the
//! CPU; §5.2 step 2 is the *simplification* the grounder does before streaming
//! aspif to clasp: drop rules with a definitely-false body, substitute facts out
//! of the surviving bodies, and compact. The instantiated rule set is one to two
//! orders of magnitude larger than the atom domain (§5.2), so this per-rule
//! filter over a huge rule set — against small `poss`/`cert` bitsets — is exactly
//! the data-parallel step the GPU targets.
//!
//! A rule `h :- pos, not neg` is kept unless a positive body atom is impossible
//! (`!poss`) or a negative one is certain (`cert`); in a kept rule, positive
//! atoms that are certain and negative atoms that are impossible are dropped
//! (they are trivially satisfied). This mirrors [`strata_asp::simplify`]'s filter
//! step; the device result is bit-identical to the CPU reference here (the
//! `poss`/`cert` fixpoints and the dedup/subsumption pass stay on the CPU).

/// A ground program in CSR form. `rules[i]` has head `head[i]` (atom id, or `-1`
/// for a constraint) and a body slice `body_lit[body_start[i]..body_start[i+1]]`.
/// A body literal is **signed**: `+(atom+1)` positive, `-(atom+1)` negative (the
/// aspif convention). `body_start` has `n+1` entries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AspCsr {
    pub head: Vec<i32>,
    pub body_start: Vec<u32>,
    pub body_lit: Vec<i32>,
}

/// Encode `(atom, positive)` as a signed body literal.
pub fn enc(atom: usize, positive: bool) -> i32 {
    let a = atom as i32 + 1;
    if positive {
        a
    } else {
        -a
    }
}
/// Decode a signed body literal into `(atom, positive)`.
pub fn dec(lit: i32) -> (usize, bool) {
    (lit.unsigned_abs() as usize - 1, lit > 0)
}

impl AspCsr {
    pub fn n_rules(&self) -> usize {
        self.head.len()
    }
}

/// Per-literal decision shared by CPU and GPU: `None` ⇒ the rule dies;
/// `Some(true)` ⇒ keep this literal; `Some(false)` ⇒ drop it (trivially true).
#[inline]
fn keep_literal(lit: i32, poss: &[u8], cert: &[u8]) -> Option<bool> {
    let (a, positive) = dec(lit);
    if positive {
        if poss[a] == 0 {
            None // positive atom impossible ⇒ body false
        } else {
            Some(cert[a] == 0) // drop if certainly true
        }
    } else if cert[a] != 0 {
        None // negative atom certain ⇒ body false
    } else {
        Some(poss[a] != 0) // drop `not impossible` = true
    }
}

/// CPU reference for the simplify-filter pass: the ground truth the GPU kernel is
/// checked against.
pub fn simplify_filter_cpu(rules: &AspCsr, poss: &[u8], cert: &[u8]) -> AspCsr {
    let mut out = AspCsr {
        head: vec![],
        body_start: vec![0],
        body_lit: vec![],
    };
    for i in 0..rules.n_rules() {
        let s = rules.body_start[i] as usize;
        let e = rules.body_start[i + 1] as usize;
        let mut kept: Vec<i32> = Vec::new();
        let mut alive = true;
        for &lit in &rules.body_lit[s..e] {
            match keep_literal(lit, poss, cert) {
                None => {
                    alive = false;
                    break;
                }
                Some(true) => kept.push(lit),
                Some(false) => {}
            }
        }
        if !alive {
            continue;
        }
        out.head.push(rules.head[i]);
        out.body_lit.extend(kept);
        out.body_start.push(out.body_lit.len() as u32);
    }
    out
}

/// Combine `poss`/`cert` into a per-atom state the kernel reads: `0` impossible,
/// `1` possible-not-certain, `2` certain.
#[cfg(feature = "cuda")]
fn atom_state(poss: &[u8], cert: &[u8]) -> Vec<u8> {
    poss.iter()
        .zip(cert)
        .map(|(&p, &c)| {
            if c != 0 {
                2
            } else if p != 0 {
                1
            } else {
                0
            }
        })
        .collect()
}

/// Simplify-filter a ground program. With the `cuda` feature this runs on the
/// GPU; without it, the CPU reference. The two are bit-identical.
pub fn simplify_filter(
    rules: &AspCsr,
    poss: &[u8],
    cert: &[u8],
) -> Result<AspCsr, crate::GpuError> {
    #[cfg(feature = "cuda")]
    {
        gpu::simplify_filter_gpu(rules, &atom_state(poss, cert))
    }
    #[cfg(not(feature = "cuda"))]
    {
        Ok(simplify_filter_cpu(rules, poss, cert))
    }
}

#[cfg(feature = "cuda")]
mod gpu {
    use super::AspCsr;
    use crate::GpuError;
    use cudarc::driver::{CudaDevice, CudaSlice, LaunchAsync, LaunchConfig};
    use cudarc::nvrtc::compile_ptx;
    use std::sync::{Arc, OnceLock};

    const BLOCK: u32 = 256;

    // `state[atom]`: 0 impossible, 1 possible, 2 certain. A signed body literal
    // `lit` decodes to atom `|lit|-1`, positive iff `lit>0`.
    const KERNELS: &str = r#"
extern "C" __global__ void scan_block(const unsigned int* in, unsigned int* out,
                                      unsigned int* blocksums, int n) {
    extern __shared__ unsigned int tmp[];
    int tid = threadIdx.x;
    int gid = blockIdx.x * blockDim.x + tid;
    tmp[tid] = (gid < n) ? in[gid] : 0u;
    __syncthreads();
    for (int off = 1; off < blockDim.x; off <<= 1) {
        unsigned int v = (tid >= off) ? tmp[tid - off] : 0u;
        __syncthreads();
        tmp[tid] += v;
        __syncthreads();
    }
    if (gid < n) out[gid] = tmp[tid] - in[gid];      // exclusive prefix sum
    if (tid == blockDim.x - 1) blocksums[blockIdx.x] = tmp[tid];
}

extern "C" __global__ void scan_add(unsigned int* out, const unsigned int* blockoff, int n) {
    int gid = blockIdx.x * blockDim.x + threadIdx.x;
    if (gid < n) out[gid] += blockoff[blockIdx.x];
}

__device__ __forceinline__ int lit_keep(int lit, const unsigned char* state, int* alive) {
    int a = (lit < 0 ? -lit : lit) - 1;
    unsigned char st = state[a];
    if (lit > 0) { if (st == 0) { *alive = 0; return 0; } return st != 2; }
    else         { if (st == 2) { *alive = 0; return 0; } return st != 0; }
}

extern "C" __global__ void filter_count(
    const unsigned int* body_start, const int* body_lit, int n,
    const unsigned char* state, unsigned int* keep, unsigned int* kept_len) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n) return;
    unsigned int s = body_start[i], e = body_start[i + 1];
    int alive = 1; unsigned int kl = 0;
    for (unsigned int k = s; k < e && alive; k++) kl += lit_keep(body_lit[k], state, &alive);
    keep[i] = alive ? 1u : 0u;
    kept_len[i] = alive ? kl : 0u;
}

extern "C" __global__ void filter_emit(
    const int* head, const unsigned int* body_start, const int* body_lit, int n,
    const unsigned char* state, const unsigned int* rank, const unsigned int* body_off,
    int* out_head, unsigned int* out_body_start, int* out_body_lit) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n) return;
    unsigned int s = body_start[i], e = body_start[i + 1];
    int alive = 1;
    for (unsigned int k = s; k < e && alive; k++) { lit_keep(body_lit[k], state, &alive); }
    if (!alive) return;
    unsigned int nr = rank[i];
    unsigned int w = body_off[i];
    out_head[nr] = head[i];
    out_body_start[nr] = w;
    int a2 = 1;
    for (unsigned int k = s; k < e; k++) {
        int keepl = lit_keep(body_lit[k], state, &a2);
        if (keepl) out_body_lit[w++] = body_lit[k];
    }
}
"#;

    fn cuda(e: impl std::fmt::Debug) -> GpuError {
        GpuError::Cuda(format!("{e:?}"))
    }

    fn device() -> Result<Arc<CudaDevice>, GpuError> {
        static DEV: OnceLock<Result<Arc<CudaDevice>, GpuError>> = OnceLock::new();
        DEV.get_or_init(|| {
            let dev = CudaDevice::new(0).map_err(cuda)?;
            let ptx = compile_ptx(KERNELS).map_err(cuda)?;
            dev.load_ptx(
                ptx,
                "asp",
                &["scan_block", "scan_add", "filter_count", "filter_emit"],
            )
            .map_err(cuda)?;
            Ok(dev)
        })
        .clone()
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

    /// Host-assisted exclusive scan of a u32 array → (positions, total).
    fn scan(
        dev: &Arc<CudaDevice>,
        flags: &CudaSlice<u32>,
        n: usize,
    ) -> Result<(CudaSlice<u32>, usize), GpuError> {
        let nb = blocks_for(n);
        let mut pos = dev.alloc_zeros::<u32>(n.max(1)).map_err(cuda)?;
        let mut blocksums = dev.alloc_zeros::<u32>(nb as usize).map_err(cuda)?;
        let scan_fn = dev.get_func("asp", "scan_block").unwrap();
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
        let add_fn = dev.get_func("asp", "scan_add").unwrap();
        unsafe {
            add_fn
                .launch(cfg(nb, 0), (&mut pos, &blockoff, n as i32))
                .map_err(cuda)?;
        }
        Ok((pos, total))
    }

    pub fn simplify_filter_gpu(rules: &AspCsr, state: &[u8]) -> Result<AspCsr, GpuError> {
        let n = rules.n_rules();
        if n == 0 {
            return Ok(AspCsr {
                head: vec![],
                body_start: vec![0],
                body_lit: vec![],
            });
        }
        let dev = device()?;

        let d_head = dev.htod_copy(rules.head.clone()).map_err(cuda)?;
        let d_bstart = dev.htod_copy(rules.body_start.clone()).map_err(cuda)?;
        let d_blit = dev.htod_copy(rules.body_lit.clone()).map_err(cuda)?;
        let d_state = dev.htod_copy(state.to_vec()).map_err(cuda)?;

        let mut keep = dev.alloc_zeros::<u32>(n).map_err(cuda)?;
        let mut kept_len = dev.alloc_zeros::<u32>(n).map_err(cuda)?;
        let count_fn = dev.get_func("asp", "filter_count").unwrap();
        unsafe {
            count_fn
                .launch(
                    cfg(blocks_for(n), 0),
                    (
                        &d_bstart,
                        &d_blit,
                        n as i32,
                        &d_state,
                        &mut keep,
                        &mut kept_len,
                    ),
                )
                .map_err(cuda)?;
        }

        let (rank, n_kept) = scan(&dev, &keep, n)?;
        let (body_off, total_body) = scan(&dev, &kept_len, n)?;

        let mut out_head = dev.alloc_zeros::<i32>(n_kept.max(1)).map_err(cuda)?;
        let mut out_bstart = dev.alloc_zeros::<u32>(n_kept + 1).map_err(cuda)?;
        let mut out_blit = dev.alloc_zeros::<i32>(total_body.max(1)).map_err(cuda)?;

        let emit_fn = dev.get_func("asp", "filter_emit").unwrap();
        unsafe {
            emit_fn
                .launch(
                    cfg(blocks_for(n), 0),
                    (
                        &d_head,
                        &d_bstart,
                        &d_blit,
                        n as i32,
                        &d_state,
                        &rank,
                        &body_off,
                        &mut out_head,
                        &mut out_bstart,
                        &mut out_blit,
                    ),
                )
                .map_err(cuda)?;
        }

        let mut head = dev.dtoh_sync_copy(&out_head).map_err(cuda)?;
        head.truncate(n_kept);
        let mut body_start = dev.dtoh_sync_copy(&out_bstart).map_err(cuda)?;
        body_start.truncate(n_kept + 1);
        body_start[n_kept] = total_body as u32;
        let mut body_lit = dev.dtoh_sync_copy(&out_blit).map_err(cuda)?;
        body_lit.truncate(total_body);

        Ok(AspCsr {
            head,
            body_start,
            body_lit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_filter_drops_and_shrinks() {
        // atoms: 0=p(cert),1=q(cert),2=s(impossible),3=r
        let poss = [1u8, 1, 0, 1];
        let cert = [1u8, 1, 0, 0];
        // r :- p, not s.   (p certain→drop; not s, s impossible→drop) ⇒ r :- .
        // r :- q,  s.      (s impossible in a positive lit → whole rule dies)
        let rules = AspCsr {
            head: vec![3, 3],
            body_start: vec![0, 2, 4],
            body_lit: vec![enc(0, true), enc(2, false), enc(1, true), enc(2, true)],
        };
        let out = simplify_filter_cpu(&rules, &poss, &cert);
        assert_eq!(out.head, vec![3]); // second rule dropped
        assert_eq!(out.body_start, vec![0, 0]); // first rule body emptied
        assert!(out.body_lit.is_empty());
    }

    #[test]
    fn enc_dec_roundtrip() {
        for atom in [0usize, 1, 5, 4095] {
            assert_eq!(dec(enc(atom, true)), (atom, true));
            assert_eq!(dec(enc(atom, false)), (atom, false));
        }
    }
}
