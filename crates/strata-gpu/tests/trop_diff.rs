//! Differential test: GPU Trop shortest paths vs a Floyd-Warshall oracle. [I5]
//!
//! Runs only with `--features cuda`. Floyd-Warshall is an independent all-pairs
//! shortest-path algorithm, unrelated to the semi-naive GPU path.
#![cfg(feature = "cuda")]

use strata_gpu::shortest_paths_trop;

const INF: i64 = i64::MAX / 4;

/// All-pairs shortest paths by Floyd-Warshall over `n` nodes. `reach(i,i)` is
/// only present when a real cycle relaxes it (init INF, no free self-path) — the
/// same convention as the Datalog `reach` relation.
fn oracle(edges: &[(u32, u32, i64)], n: usize) -> Vec<(u32, u32, i64)> {
    let mut d = vec![vec![INF; n]; n];
    for &(x, y, w) in edges {
        let (x, y) = (x as usize, y as usize);
        if w < d[x][y] {
            d[x][y] = w;
        }
    }
    for k in 0..n {
        for i in 0..n {
            if d[i][k] == INF {
                continue;
            }
            for j in 0..n {
                if d[k][j] != INF && d[i][k] + d[k][j] < d[i][j] {
                    d[i][j] = d[i][k] + d[k][j];
                }
            }
        }
    }
    let mut out = Vec::new();
    for (i, row) in d.iter().enumerate() {
        for (j, &w) in row.iter().enumerate() {
            if w < INF {
                out.push((i as u32, j as u32, w));
            }
        }
    }
    out.sort_unstable();
    out
}

fn node_count(edges: &[(u32, u32, i64)]) -> usize {
    edges
        .iter()
        .flat_map(|&(a, b, _)| [a, b])
        .max()
        .map_or(0, |m| m as usize + 1)
}

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
    // two routes a→c: direct 10, via b 2+3=5 ⇒ 5 wins
    let g = vec![(0, 2, 10), (0, 1, 2), (1, 2, 3)];
    let got = shortest_paths_trop(&g).unwrap();
    assert!(got.contains(&(0, 2, 5)), "{got:?}");
    assert_eq!(got, oracle(&g, node_count(&g)));

    // empty
    assert_eq!(
        shortest_paths_trop(&[]).unwrap(),
        Vec::<(u32, u32, i64)>::new()
    );
}

#[test]
fn matches_floyd_warshall_random() {
    let mut rng = Rng(0xdead_beef_cafe_1234);
    for trial in 0..200u32 {
        let nodes = 3 + rng.upto(6) as u32; // 3..=8 nodes
        let m = rng.upto(nodes as u64 * 2 + 1) as usize;
        let mut edges = Vec::with_capacity(m);
        for _ in 0..m {
            let a = rng.upto(nodes as u64) as u32;
            let b = rng.upto(nodes as u64) as u32;
            let w = rng.upto(10) as i64; // non-negative weights
            edges.push((a, b, w));
        }
        let got = shortest_paths_trop(&edges).expect("gpu trop");
        let want = oracle(&edges, node_count(&edges));
        assert_eq!(got, want, "trial {trial}, edges {edges:?}");
    }
}

#[test]
fn weighted_chain() {
    // 0→1→…→N with edge i→i+1 of weight (i+1). reach(i,j) = sum_{k=i+1..=j} k.
    let n: u32 = 300;
    let edges: Vec<(u32, u32, i64)> = (0..n).map(|i| (i, i + 1, (i + 1) as i64)).collect();
    let got = shortest_paths_trop(&edges).expect("gpu trop");

    let mut want = Vec::new();
    for i in 0..=n {
        let mut acc = 0i64;
        for j in (i + 1)..=n {
            acc += j as i64;
            want.push((i, j, acc));
        }
    }
    assert_eq!(got, want);
}
