//! Device-resident Bool transitive closure. [Phase 1, slice 8]
//!
//! `path` and `delta` stay on the GPU across rounds as sorted `(src,dst)`=`u64`
//! key arrays. Each semi-naive round runs entirely on the device:
//!
//! 1. join `E ⋈ Δ` (two-phase count→emit) producing encoded result keys,
//! 2. radix-sort + dedup that result,
//! 3. subtract the tuples already in `path` (in-kernel binary search),
//! 4. merge the fresh tuples into `path` (concat + radix sort).
//!
//! No relation round-trips through the host — only each round's *fresh count*
//! (to test the fixpoint) and the final result are copied back. The join
//! binary-searches `Δ`'s `src` (the high 32 bits of its sorted keys). All
//! kernels live in the shared [`crate::sort`] device module.

use cudarc::driver::{LaunchAsync, LaunchConfig};

use crate::sort;
use crate::GpuError;

fn cuda<E: std::fmt::Display>(e: E) -> GpuError {
    GpuError::Cuda(e.to_string())
}

fn normalize(mut rel: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    rel.sort_unstable();
    rel.dedup();
    rel
}

pub fn transitive_closure_bool(edges: &[(u32, u32)]) -> Result<Vec<(u32, u32)>, GpuError> {
    let e = normalize(edges.to_vec());
    if e.is_empty() {
        return Ok(Vec::new());
    }
    let dev = sort::shared_device()?;
    let ne = e.len();

    // Constant edge columns.
    let esrc = dev
        .htod_copy(e.iter().map(|p| p.0).collect::<Vec<u32>>())
        .map_err(cuda)?;
    let edst = dev
        .htod_copy(e.iter().map(|p| p.1).collect::<Vec<u32>>())
        .map_err(cuda)?;

    // path and delta as sorted (src,dst) keys, resident on the device.
    let e_keys: Vec<u64> = e
        .iter()
        .map(|&(s, d)| ((s as u64) << 32) | d as u64)
        .collect();
    let mut path = dev.htod_copy(e_keys.clone()).map_err(cuda)?;
    let mut npath = ne;
    let mut delta = dev.htod_copy(e_keys).map_err(cuda)?;
    let mut ndelta = ne;

    let ecfg = LaunchConfig::for_num_elems(ne as u32);

    loop {
        // --- join E ⋈ Δ (two-phase) → raw result keys on device ---
        let mut counts = dev.alloc_zeros::<u32>(ne).map_err(cuda)?;
        let cf = dev.get_func("sort", "bj_count").unwrap();
        unsafe {
            cf.launch(ecfg, (&edst, ne as i32, &delta, ndelta as i32, &mut counts))
                .map_err(cuda)?;
        }
        let counts_h = dev.dtoh_sync_copy(&counts).map_err(cuda)?;
        let mut offsets_h = vec![0u32; ne];
        let mut total: u64 = 0;
        for i in 0..ne {
            offsets_h[i] = total as u32;
            total += counts_h[i] as u64;
        }
        if total == 0 {
            break;
        }
        let total = total as usize;
        let offsets = dev.htod_copy(offsets_h).map_err(cuda)?;
        let mut raw = dev.alloc_zeros::<u64>(total).map_err(cuda)?;
        let ef = dev.get_func("sort", "bj_emit").unwrap();
        unsafe {
            ef.launch(
                ecfg,
                (
                    &esrc,
                    &edst,
                    ne as i32,
                    &delta,
                    ndelta as i32,
                    &offsets,
                    &mut raw,
                ),
            )
            .map_err(cuda)?;
        }

        // --- sort + dedup the join result on device ---
        let (cand, ncand) = sort::sort_unique_dev(&dev, raw, total)?;

        // --- fresh = cand \ path (in-kernel binary search + compact) ---
        let mut flags = dev.alloc_zeros::<u32>(ncand).map_err(cuda)?;
        let df = dev.get_func("sort", "diff_flag").unwrap();
        unsafe {
            df.launch(
                LaunchConfig::for_num_elems(ncand as u32),
                (&cand, ncand as i32, &path, npath as i32, &mut flags),
            )
            .map_err(cuda)?;
        }
        let (pos, nfresh) = sort::scan_flags(&dev, &flags, ncand)?;
        if nfresh == 0 {
            break;
        }
        let mut fresh = dev.alloc_zeros::<u64>(nfresh).map_err(cuda)?;
        let sf = dev.get_func("sort", "scatter_unique").unwrap();
        unsafe {
            sf.launch(
                LaunchConfig::for_num_elems(ncand as u32),
                (&cand, &flags, &pos, ncand as i32, &mut fresh),
            )
            .map_err(cuda)?;
        }

        // --- path ∪ fresh (disjoint, both sorted) → radix-sort the concat ---
        let ntot = npath + nfresh;
        let mut comb = dev.alloc_zeros::<u64>(ntot).map_err(cuda)?;
        let cp = dev.get_func("sort", "copy_u64").unwrap();
        unsafe {
            cp.launch(
                LaunchConfig::for_num_elems(npath as u32),
                (&path, npath as i32, &mut comb, 0i32),
            )
            .map_err(cuda)?;
            let cp2 = dev.get_func("sort", "copy_u64").unwrap();
            cp2.launch(
                LaunchConfig::for_num_elems(nfresh as u32),
                (&fresh, nfresh as i32, &mut comb, npath as i32),
            )
            .map_err(cuda)?;
        }
        path = sort::radix_sort_u64(&dev, comb, ntot)?;
        npath = ntot;
        delta = fresh;
        ndelta = nfresh;
    }

    let mut keys = dev.dtoh_sync_copy(&path).map_err(cuda)?;
    keys.truncate(npath);
    Ok(keys
        .into_iter()
        .map(|k| ((k >> 32) as u32, (k & 0xffff_ffff) as u32))
        .collect())
}
