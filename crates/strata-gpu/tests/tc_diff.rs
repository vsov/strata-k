//! Differential test: GPU transitive closure vs an independent CPU oracle. [I5]
//!
//! Runs only with `--features cuda` (needs the GPU). The oracle is a dead-simple
//! set fixpoint, obviously correct and independent of the GPU code path.
#![cfg(feature = "cuda")]

use std::collections::BTreeSet;

use strata_gpu::transitive_closure_bool;

/// Obviously-correct transitive closure: close the pair set under composition.
fn oracle(edges: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut set: BTreeSet<(u32, u32)> = edges.iter().copied().collect();
    loop {
        let mut add = Vec::new();
        for &(x, y) in &set {
            for &(y2, z) in &set {
                if y2 == y && !set.contains(&(x, z)) {
                    add.push((x, z));
                }
            }
        }
        if add.is_empty() {
            break;
        }
        set.extend(add);
    }
    set.into_iter().collect()
}

/// Tiny deterministic LCG so trials are reproducible without a dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn upto(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[test]
fn fixed_cases() {
    // chain a→b→c→d
    let chain = vec![(0, 1), (1, 2), (2, 3)];
    assert_eq!(transitive_closure_bool(&chain).unwrap(), oracle(&chain));

    // 2-cycle a→b→a closes to the full 2×2
    let cyc = vec![(0, 1), (1, 0)];
    assert_eq!(transitive_closure_bool(&cyc).unwrap(), oracle(&cyc));

    // self-loop + duplicate inputs
    let dup = vec![(5, 5), (5, 5), (5, 6)];
    assert_eq!(transitive_closure_bool(&dup).unwrap(), oracle(&dup));

    // empty
    assert_eq!(
        transitive_closure_bool(&[]).unwrap(),
        Vec::<(u32, u32)>::new()
    );
}

#[test]
fn matches_oracle_on_random_graphs() {
    let mut rng = Rng(0x1234_5678_9abc_def0);
    for trial in 0..200u32 {
        let nodes = 3 + rng.upto(8) as u32; // 3..=10 nodes
        let m = rng.upto(nodes as u64 * 2 + 1) as usize;
        let mut edges = Vec::with_capacity(m);
        for _ in 0..m {
            let a = rng.upto(nodes as u64) as u32;
            let b = rng.upto(nodes as u64) as u32;
            edges.push((a, b));
        }
        let got = transitive_closure_bool(&edges).expect("gpu tc");
        let want = oracle(&edges);
        assert_eq!(got, want, "trial {trial}, edges {edges:?}");
    }
}

#[test]
fn chain_scale_semi_naive() {
    // A 400-edge chain 0→1→…→400 forces 400 deep semi-naive rounds and closes to
    // exactly {(i, j) : i < j}. Checks scale + iteration depth against a closed
    // form, without the O(n²) set oracle.
    let n: u32 = 400;
    let edges: Vec<(u32, u32)> = (0..n).map(|i| (i, i + 1)).collect();
    let got = transitive_closure_bool(&edges).expect("gpu tc");

    let mut want = Vec::new();
    for i in 0..=n {
        for j in (i + 1)..=n {
            want.push((i, j));
        }
    }
    assert_eq!(got.len(), (n as usize + 1) * n as usize / 2);
    assert_eq!(got, want);
}
