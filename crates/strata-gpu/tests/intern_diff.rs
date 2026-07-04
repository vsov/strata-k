//! Differential test: GPU device interning vs a host interner. [Phase 3, I5]
//!
//! Runs only with `--features cuda`. Interned ids are labeled by sorted order on
//! the GPU and by first-occurrence on the host, so we check the *partition* (two
//! inputs share an id iff they share a key) and the distinct count — the
//! labeling-agnostic invariants an interner must satisfy.
#![cfg(feature = "cuda")]

use std::collections::HashMap;

use strata_gpu::intern_terms;

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

fn check(keys: &[u64]) {
    let ids = intern_terms(keys).expect("gpu intern");
    assert_eq!(ids.len(), keys.len());

    // Same key ⇔ same id, and ids are a dense relabeling of the distinct keys.
    let mut key_to_id: HashMap<u64, u32> = HashMap::new();
    let mut id_to_key: HashMap<u32, u64> = HashMap::new();
    for (&k, &id) in keys.iter().zip(&ids) {
        assert_eq!(*key_to_id.entry(k).or_insert(id), id, "key {k} got two ids");
        assert_eq!(*id_to_key.entry(id).or_insert(k), k, "id {id} for two keys");
    }
    let distinct = key_to_id.len();
    // Dense id space: the sorted distinct ids are exactly 0..distinct.
    let mut seen: Vec<u32> = ids.clone();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), distinct);
    if distinct > 0 {
        assert_eq!(seen.first(), Some(&0));
        assert_eq!(seen.last(), Some(&(distinct as u32 - 1)));
    }
}

#[test]
fn fixed_cases() {
    check(&[]);
    check(&[42]);
    check(&[7, 7, 7]);
    check(&[3, 1, 2, 1, 3, 3, 2]);
}

#[test]
fn matches_host_partition_random() {
    let mut rng = Rng(0x1a7e_9999);
    for _ in 0..100 {
        let n = (rng.next() % 5000) as usize;
        let keys: Vec<u64> = (0..n).map(|_| rng.next() % 200).collect(); // heavy dups
        check(&keys);
    }
}
