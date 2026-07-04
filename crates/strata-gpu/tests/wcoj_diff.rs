//! Differential test: GPU WCOJ triangle count vs a brute-force oracle. [I5]
//!
//! Runs only with `--features cuda`. The oracle checks all `a<b<c` triples, an
//! independent O(n³) method unrelated to the leapfrog GPU path.
#![cfg(feature = "cuda")]

use std::collections::HashSet;

use strata_gpu::count_triangles;

fn edge_set(edges: &[(u32, u32)]) -> HashSet<(u32, u32)> {
    edges
        .iter()
        .filter_map(|&(a, b)| match a.cmp(&b) {
            std::cmp::Ordering::Less => Some((a, b)),
            std::cmp::Ordering::Greater => Some((b, a)),
            std::cmp::Ordering::Equal => None,
        })
        .collect()
}

/// Triangles by checking every triple — obviously correct, O(n³).
fn oracle(edges: &[(u32, u32)], n: u32) -> u64 {
    let set = edge_set(edges);
    let mut t = 0u64;
    for a in 0..n {
        for b in (a + 1)..n {
            if !set.contains(&(a, b)) {
                continue;
            }
            for c in (b + 1)..n {
                if set.contains(&(a, c)) && set.contains(&(b, c)) {
                    t += 1;
                }
            }
        }
    }
    t
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
    assert_eq!(count_triangles(&[]).unwrap(), 0);
    // one triangle
    assert_eq!(count_triangles(&[(0, 1), (1, 2), (0, 2)]).unwrap(), 1);
    // K4 has C(4,3) = 4 triangles; feed both directions + a duplicate + self-loop
    let k4 = [
        (0, 1),
        (1, 0),
        (0, 2),
        (0, 3),
        (1, 2),
        (2, 3),
        (1, 3),
        (1, 3),
        (2, 2),
    ];
    assert_eq!(count_triangles(&k4).unwrap(), 4);
}

#[test]
fn complete_graphs() {
    // K_n has C(n,3) triangles.
    for n in [3u32, 10, 30, 64] {
        let mut edges = Vec::new();
        for a in 0..n {
            for b in (a + 1)..n {
                edges.push((a, b));
            }
        }
        let expected = (n as u64) * (n as u64 - 1) * (n as u64 - 2) / 6;
        assert_eq!(count_triangles(&edges).unwrap(), expected, "K{n}");
    }
}

#[test]
fn matches_bruteforce_random() {
    let mut rng = Rng(0x7ea_1234_5678);
    for trial in 0..200u32 {
        let n = 5 + rng.upto(30) as u32; // 5..=34 nodes
        let m = rng.upto(n as u64 * 3) as usize;
        let mut edges = Vec::with_capacity(m);
        for _ in 0..m {
            let a = rng.upto(n as u64) as u32;
            let b = rng.upto(n as u64) as u32;
            edges.push((a, b));
        }
        let got = count_triangles(&edges).expect("gpu tri");
        let want = oracle(&edges, n);
        assert_eq!(got, want, "trial {trial}, edges {edges:?}");
    }
}

/// 4-cliques by checking every 4-tuple — obviously correct, O(n⁴).
fn oracle4(edges: &[(u32, u32)], n: u32) -> u64 {
    let set = edge_set(edges);
    let e = |a: u32, b: u32| set.contains(&(a.min(b), a.max(b)));
    let mut t = 0u64;
    for a in 0..n {
        for b in (a + 1)..n {
            if !e(a, b) {
                continue;
            }
            for c in (b + 1)..n {
                if !(e(a, c) && e(b, c)) {
                    continue;
                }
                for d in (c + 1)..n {
                    if e(a, d) && e(b, d) && e(c, d) {
                        t += 1;
                    }
                }
            }
        }
    }
    t
}

#[test]
fn fourcliques_fixed_and_complete() {
    use strata_gpu::count_4cliques;
    // K4 itself → 1
    assert_eq!(
        count_4cliques(&[(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)]).unwrap(),
        1
    );
    // a triangle has no 4-clique
    assert_eq!(count_4cliques(&[(0, 1), (1, 2), (0, 2)]).unwrap(), 0);
    // K_n → C(n,4)
    for n in [4u32, 10, 20, 40] {
        let mut e = Vec::new();
        for a in 0..n {
            for b in (a + 1)..n {
                e.push((a, b));
            }
        }
        let exp = (n as u64) * (n as u64 - 1) * (n as u64 - 2) * (n as u64 - 3) / 24;
        assert_eq!(count_4cliques(&e).unwrap(), exp, "K{n}");
    }
}

#[test]
fn fourcliques_match_bruteforce() {
    use strata_gpu::count_4cliques;
    let mut rng = Rng(0x4c1_9999);
    for trial in 0..100u32 {
        let n = 6 + rng.upto(18) as u32; // 6..=23 nodes (O(n⁴) oracle)
        let m = rng.upto(n as u64 * 4) as usize;
        let mut edges = Vec::with_capacity(m);
        for _ in 0..m {
            edges.push((rng.upto(n as u64) as u32, rng.upto(n as u64) as u32));
        }
        let got = count_4cliques(&edges).expect("gpu k4");
        let want = oracle4(&edges, n);
        assert_eq!(got, want, "trial {trial}, edges {edges:?}");
    }
}
