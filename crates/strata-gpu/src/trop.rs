//! Device-resident Trop (min-plus) shortest paths. [Phase 1, slice 9]
//!
//! The min-plus counterpart of the Bool closure: `⊗` is `+` (path weight) and
//! `⊕` is `min`. `reach(X,Z)` carries the least total weight of any `X→Z` path.
//! `path` and `delta` stay on the GPU across rounds as sorted `(src,dst)`=`u64`
//! keys plus a parallel `i64` weight column. Each semi-naive round runs on the
//! device:
//!
//! 1. join `E ⋈ Δ` → `(E.src, Δ.dst, E.w + Δ.w)` (two-phase count→emit),
//! 2. min-aggregate the result by key (device kv-sort + segmented min),
//! 3. keep the tuples that are new or strictly improve `path`'s weight
//!    (in-kernel binary search + compare), then merge them into `path` by
//!    re-aggregating `path ∪ fresh` (a smaller improved weight wins).
//!
//! Non-negative weights guarantee termination; a Bellman-Ford round cap turns a
//! negative-weight cycle into an error instead of a hang. Only the per-round
//! fresh-count and the final result cross to the host.

use cudarc::driver::{LaunchAsync, LaunchConfig};

use crate::sort;
use crate::GpuError;

fn cuda<E: std::fmt::Display>(e: E) -> GpuError {
    GpuError::Cuda(e.to_string())
}

fn key_of(s: u32, d: u32) -> u64 {
    ((s as u64) << 32) | d as u64
}

/// Least weight per `(src, dst)` from the input edges, via a host sort + linear
/// run-collapse (scale-friendly, unlike a per-edge map).
fn best_of_sorted(edges: &[(u32, u32, i64)]) -> Vec<(u32, u32, i64)> {
    let mut v = edges.to_vec();
    v.sort_unstable_by_key(|&(s, d, _)| key_of(s, d));
    let mut out: Vec<(u32, u32, i64)> = Vec::new();
    for &(s, d, w) in &v {
        match out.last_mut() {
            Some(last) if last.0 == s && last.1 == d => {
                if w < last.2 {
                    last.2 = w;
                }
            }
            _ => out.push((s, d, w)),
        }
    }
    out
}

pub fn shortest_paths_trop(edges: &[(u32, u32, i64)]) -> Result<Vec<(u32, u32, i64)>, GpuError> {
    let base = best_of_sorted(edges);
    if base.is_empty() {
        return Ok(Vec::new());
    }
    let dev = sort::shared_device()?;
    let ne = base.len();

    // Constant edge columns.
    let esrc = dev
        .htod_copy(base.iter().map(|t| t.0).collect::<Vec<u32>>())
        .map_err(cuda)?;
    let edst = dev
        .htod_copy(base.iter().map(|t| t.1).collect::<Vec<u32>>())
        .map_err(cuda)?;
    let ew = dev
        .htod_copy(base.iter().map(|t| t.2).collect::<Vec<i64>>())
        .map_err(cuda)?;

    // path and delta as sorted keys + weights, resident on the device.
    let base_keys: Vec<u64> = base.iter().map(|&(s, d, _)| key_of(s, d)).collect();
    let base_w: Vec<i64> = base.iter().map(|t| t.2).collect();
    let mut pathk = dev.htod_copy(base_keys.clone()).map_err(cuda)?;
    let mut pathw = dev.htod_copy(base_w.clone()).map_err(cuda)?;
    let mut npath = ne;
    let mut dk = dev.htod_copy(base_keys).map_err(cuda)?;
    let mut dw = dev.htod_copy(base_w).map_err(cuda)?;
    let mut ndelta = ne;

    let ecfg = LaunchConfig::for_num_elems(ne as u32);
    let cap = ne + 2; // Bellman-Ford bound: ≤ ne improvement rounds w/o a neg cycle
    let mut round = 0usize;

    loop {
        round += 1;
        if round > cap {
            return Err(GpuError::Cuda(
                "negative-weight cycle in Trop closure".into(),
            ));
        }

        // --- join E ⋈ Δ → (key, weight) on device ---
        let mut counts = dev.alloc_zeros::<u32>(ne).map_err(cuda)?;
        let cf = dev.get_func("sort", "bj_count").unwrap();
        unsafe {
            cf.launch(ecfg, (&edst, ne as i32, &dk, ndelta as i32, &mut counts))
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
        let mut rawk = dev.alloc_zeros::<u64>(total).map_err(cuda)?;
        let mut raww = dev.alloc_zeros::<i64>(total).map_err(cuda)?;
        let ef = dev.get_func("sort", "bjt_emit").unwrap();
        unsafe {
            ef.launch(
                ecfg,
                (
                    &esrc,
                    &edst,
                    &ew,
                    ne as i32,
                    &dk,
                    &dw,
                    ndelta as i32,
                    &offsets,
                    &mut rawk,
                    &mut raww,
                ),
            )
            .map_err(cuda)?;
        }

        // --- min-aggregate the join result by key ---
        let (candk, candw, ncand) = sort::sort_min_dev(&dev, rawk, raww, total)?;

        // --- keep new / strictly-improving tuples ---
        let mut flags = dev.alloc_zeros::<u32>(ncand).map_err(cuda)?;
        let di = dev.get_func("sort", "diff_improve").unwrap();
        unsafe {
            di.launch(
                LaunchConfig::for_num_elems(ncand as u32),
                (
                    &candk,
                    &candw,
                    ncand as i32,
                    &pathk,
                    &pathw,
                    npath as i32,
                    &mut flags,
                ),
            )
            .map_err(cuda)?;
        }
        let (pos, nfresh) = sort::scan_flags(&dev, &flags, ncand)?;
        if nfresh == 0 {
            break;
        }
        let mut freshk = dev.alloc_zeros::<u64>(nfresh).map_err(cuda)?;
        let mut freshw = dev.alloc_zeros::<i64>(nfresh).map_err(cuda)?;
        let scv = dev.get_func("sort", "scatter_unique_kv").unwrap();
        unsafe {
            scv.launch(
                LaunchConfig::for_num_elems(ncand as u32),
                (
                    &candk,
                    &candw,
                    &flags,
                    &pos,
                    ncand as i32,
                    &mut freshk,
                    &mut freshw,
                ),
            )
            .map_err(cuda)?;
        }

        // --- path = min-aggregate(path ∪ fresh) (improved weight wins) ---
        let ntot = npath + nfresh;
        let mut combk = dev.alloc_zeros::<u64>(ntot).map_err(cuda)?;
        let mut combw = dev.alloc_zeros::<i64>(ntot).map_err(cuda)?;
        let cu = dev.get_func("sort", "copy_u64").unwrap();
        let ci = dev.get_func("sort", "copy_i64").unwrap();
        unsafe {
            cu.launch(
                LaunchConfig::for_num_elems(npath as u32),
                (&pathk, npath as i32, &mut combk, 0i32),
            )
            .map_err(cuda)?;
            ci.launch(
                LaunchConfig::for_num_elems(npath as u32),
                (&pathw, npath as i32, &mut combw, 0i32),
            )
            .map_err(cuda)?;
            let cu2 = dev.get_func("sort", "copy_u64").unwrap();
            let ci2 = dev.get_func("sort", "copy_i64").unwrap();
            cu2.launch(
                LaunchConfig::for_num_elems(nfresh as u32),
                (&freshk, nfresh as i32, &mut combk, npath as i32),
            )
            .map_err(cuda)?;
            ci2.launch(
                LaunchConfig::for_num_elems(nfresh as u32),
                (&freshw, nfresh as i32, &mut combw, npath as i32),
            )
            .map_err(cuda)?;
        }
        let (nk, nw, nn) = sort::sort_min_dev(&dev, combk, combw, ntot)?;
        pathk = nk;
        pathw = nw;
        npath = nn;
        dk = freshk;
        dw = freshw;
        ndelta = nfresh;
    }

    let mut keys = dev.dtoh_sync_copy(&pathk).map_err(cuda)?;
    let mut wts = dev.dtoh_sync_copy(&pathw).map_err(cuda)?;
    keys.truncate(npath);
    wts.truncate(npath);
    Ok(keys
        .into_iter()
        .zip(wts)
        .map(|(k, w)| ((k >> 32) as u32, (k & 0xffff_ffff) as u32, w))
        .collect())
}
