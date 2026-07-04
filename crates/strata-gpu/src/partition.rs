//! Multi-GPU groundwork: the partitioning interfaces. [Phase 6, spec §7]
//!
//! Spec §7: a single GPU can't hold the motivating knowledge graphs; v1 ships
//! only the *partitioning interface* (shardable columns), no distributed
//! execution — but the interfaces are cheaper to design now. Two are the load-
//! bearing ones:
//!
//! - **Radix hash-partition** by join key: a binary join `R ⋈ S` on a key splits
//!   into `k` independent shards because tuples that could match hash to the same
//!   shard; the whole join is the union of the per-shard joins (the delta-only
//!   NVLink exchange in the spec).
//! - **HCube** (hypercube) partition for a worst-case-optimal multiway join: give
//!   each variable a number of slots, replicate each relation's tuples to the
//!   hypercube cells consistent with its bound variables, run a local WCOJ per
//!   cell, and union. Minimizes replication for skew.
//!
//! Pure CPU reference: each function's result is proved equal to the un-
//! partitioned computation, so a future distributed executor has a spec to match.

/// Deterministic hash used for sharding (splitmix-style finalizer).
fn hash(x: i64) -> u64 {
    let mut z = (x as u64).wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// Which of `p` slots a value maps to.
pub fn slot(x: i64, p: usize) -> usize {
    (hash(x) % p as u64) as usize
}

/// Partition tuples into `k` shards by the hash of `key_cols` (a shardable-column
/// view). Concatenating the shards recovers the input (a pure repartition).
pub fn hash_partition(tuples: &[Vec<i64>], key_cols: &[usize], k: usize) -> Vec<Vec<Vec<i64>>> {
    let mut parts = vec![Vec::new(); k];
    for t in tuples {
        let mut h = 0u64;
        for &c in key_cols {
            h = h.wrapping_mul(1099511628211) ^ hash(t[c]);
        }
        parts[(h % k as u64) as usize].push(t.clone());
    }
    parts
}

/// Direct hash join of binary relations on one column each: pairs `(a, b, c)`
/// where `r(a, b_r)` and `s(b_s, c)` with `r[r_key] == s[s_key]`.
fn binary_join(r: &[Vec<i64>], s: &[Vec<i64>], r_key: usize, s_key: usize) -> Vec<(i64, i64, i64)> {
    use std::collections::HashMap;
    let mut idx: HashMap<i64, Vec<&Vec<i64>>> = HashMap::new();
    for t in s {
        idx.entry(t[s_key]).or_default().push(t);
    }
    let mut out = Vec::new();
    for tr in r {
        if let Some(ss) = idx.get(&tr[r_key]) {
            for ts in ss {
                out.push((tr[0], tr[r_key], ts[1 - s_key]));
            }
        }
    }
    out
}

/// Radix-partitioned binary join on the join key: partition both sides by the key
/// into `k` shards, join each shard locally, concatenate. Equal (as a multiset)
/// to [`binary_join`] — the correctness the interface promises.
pub fn partitioned_binary_join(
    r: &[Vec<i64>],
    s: &[Vec<i64>],
    r_key: usize,
    s_key: usize,
    k: usize,
) -> Vec<(i64, i64, i64)> {
    let rp = hash_partition(r, &[r_key], k);
    let sp = hash_partition(s, &[s_key], k);
    let mut out = Vec::new();
    for shard in 0..k {
        out.extend(binary_join(&rp[shard], &sp[shard], r_key, s_key));
    }
    out
}

/// Directed-triangle count over an edge relation (undirected pairs both ways is
/// the caller's choice): `#{(a,b,c) : (a,b),(b,c),(a,c) ∈ E}`.
pub fn triangles(edges: &[Vec<i64>]) -> u64 {
    use std::collections::HashSet;
    let set: HashSet<(i64, i64)> = edges.iter().map(|e| (e[0], e[1])).collect();
    let mut count = 0u64;
    for ab in edges {
        for bc in edges {
            if bc[0] == ab[1] && set.contains(&(ab[0], bc[1])) {
                count += 1;
            }
        }
    }
    count
}

/// HCube triangle count over `p` slots per variable (`p³` cells): each edge is
/// replicated to the cells consistent with its two bound variables, a local
/// triangle join runs per cell, and the counts sum. Each triangle lands in
/// exactly one cell, so the total equals [`triangles`].
pub fn hypercube_triangles(edges: &[Vec<i64>], p: usize) -> u64 {
    use std::collections::HashSet;
    let mut total = 0u64;
    for i in 0..p {
        for j in 0..p {
            for k in 0..p {
                // R(a,b): a→i, b→j ; S(b,c): b→j, c→k ; T(a,c): a→i, c→k.
                let rab: Vec<&Vec<i64>> = edges
                    .iter()
                    .filter(|e| slot(e[0], p) == i && slot(e[1], p) == j)
                    .collect();
                let tac: HashSet<(i64, i64)> = edges
                    .iter()
                    .filter(|e| slot(e[0], p) == i && slot(e[1], p) == k)
                    .map(|e| (e[0], e[1]))
                    .collect();
                let sbc: Vec<&Vec<i64>> = edges
                    .iter()
                    .filter(|e| slot(e[0], p) == j && slot(e[1], p) == k)
                    .collect();
                // local join: a-b (rab) with b-c (sbc), check a-c in tac.
                for ab in &rab {
                    for bc in &sbc {
                        if bc[0] == ab[1] && tac.contains(&(ab[0], bc[1])) {
                            total += 1;
                        }
                    }
                }
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn ms(v: &[(i64, i64, i64)]) -> BTreeMap<(i64, i64, i64), usize> {
        let mut m = BTreeMap::new();
        for &t in v {
            *m.entry(t).or_insert(0) += 1;
        }
        m
    }

    #[test]
    fn hash_partition_is_lossless() {
        let tuples: Vec<Vec<i64>> = (0..50).map(|i| vec![i, i * 7 % 13]).collect();
        let parts = hash_partition(&tuples, &[0], 8);
        let total: usize = parts.iter().map(|p| p.len()).sum();
        assert_eq!(total, tuples.len());
        // same-key tuples always co-locate.
        let mut seen: std::collections::HashMap<i64, usize> = Default::default();
        for (pi, part) in parts.iter().enumerate() {
            for t in part {
                if let Some(&prev) = seen.get(&t[0]) {
                    assert_eq!(prev, pi, "key {} split across shards", t[0]);
                }
                seen.insert(t[0], pi);
            }
        }
    }

    #[test]
    fn partitioned_join_equals_whole() {
        let r: Vec<Vec<i64>> = (0..20).map(|i| vec![i, i % 5]).collect();
        let s: Vec<Vec<i64>> = (0..20).map(|i| vec![i % 5, i * 3]).collect();
        for k in [1usize, 2, 4, 7] {
            let whole = ms(&binary_join(&r, &s, 1, 0));
            let parted = ms(&partitioned_binary_join(&r, &s, 1, 0, k));
            assert_eq!(whole, parted, "radix k={k} changed the join");
        }
    }

    #[test]
    fn hypercube_triangles_equal_whole() {
        // a small directed graph with several triangles.
        let edges: Vec<Vec<i64>> = vec![
            vec![0, 1],
            vec![1, 2],
            vec![0, 2],
            vec![2, 3],
            vec![1, 3],
            vec![0, 3],
            vec![3, 0],
            vec![2, 0],
        ];
        let whole = triangles(&edges);
        for p in [1usize, 2, 3] {
            assert_eq!(
                hypercube_triangles(&edges, p),
                whole,
                "HCube p={p} miscounts triangles"
            );
        }
    }

    #[test]
    fn hypercube_random_matches() {
        let mut seed = 0xC0FFEEu64;
        let mut nxt = |m: i64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 33) as i64 % m
        };
        for _ in 0..30 {
            let mut es: std::collections::BTreeSet<(i64, i64)> = Default::default();
            for _ in 0..14 {
                es.insert((nxt(6), nxt(6)));
            }
            let edges: Vec<Vec<i64>> = es.iter().map(|&(a, b)| vec![a, b]).collect();
            let whole = triangles(&edges);
            assert_eq!(hypercube_triangles(&edges, 3), whole);
        }
    }
}
