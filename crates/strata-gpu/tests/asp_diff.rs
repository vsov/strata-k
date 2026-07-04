//! Differential test for the GPU grounding-simplification filter (spec §5.2):
//! the device result must be bit-identical to the CPU reference on random ground
//! programs. Only built with the `cuda` feature (needs a GPU).

#![cfg(feature = "cuda")]

use strata_gpu::asp::{enc, simplify_filter, simplify_filter_cpu, AspCsr};

/// Deterministic LCG so the corpus is reproducible without an RNG crate.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
    fn upto(&mut self, n: usize) -> usize {
        (self.next() >> 11) as usize % n
    }
}

/// Build a random ground program over `n_atoms` with up to `max_body` body lits,
/// plus random `poss`/`cert` bitsets (with `cert ⊆ poss`, as the fixpoints give).
fn random(
    seed: u64,
    n_rules: usize,
    n_atoms: usize,
    max_body: usize,
) -> (AspCsr, Vec<u8>, Vec<u8>) {
    let mut rng = Rng(seed);
    let mut csr = AspCsr {
        head: vec![],
        body_start: vec![0],
        body_lit: vec![],
    };
    for _ in 0..n_rules {
        // 1/8 of rules are constraints (head -1).
        let head = if rng.upto(8) == 0 {
            -1
        } else {
            rng.upto(n_atoms) as i32
        };
        csr.head.push(head);
        let blen = rng.upto(max_body + 1);
        for _ in 0..blen {
            csr.body_lit.push(enc(rng.upto(n_atoms), rng.upto(2) == 1));
        }
        csr.body_start.push(csr.body_lit.len() as u32);
    }
    let poss: Vec<u8> = (0..n_atoms).map(|_| (rng.upto(4) != 0) as u8).collect(); // ~75% possible
    let cert: Vec<u8> = (0..n_atoms)
        .map(|i| (poss[i] == 1 && rng.upto(3) == 0) as u8) // certain ⊆ possible
        .collect();
    (csr, poss, cert)
}

#[test]
fn gpu_filter_matches_cpu_reference() {
    for seed in 0..200u64 {
        let (csr, poss, cert) = random(seed.wrapping_mul(0x9E3779B9) + 1, 64, 24, 5);
        let cpu = simplify_filter_cpu(&csr, &poss, &cert);
        let gpu = simplify_filter(&csr, &poss, &cert).expect("gpu");
        assert_eq!(cpu, gpu, "seed {seed}: GPU filter != CPU reference");
    }
}

#[test]
fn gpu_filter_scales_and_reports_throughput() {
    // One large instantiated rule set — the §5.2 case where rule instantiations
    // dwarf the atom domain. Bit-exact vs CPU + a rules/sec throughput number
    // (the exit's "grounding-phase speedup" metric).
    let n_rules = 4_000_000;
    let (csr, poss, cert) = random(0xA5A5, n_rules, 4096, 6);

    let t0 = std::time::Instant::now();
    let gpu = simplify_filter(&csr, &poss, &cert).expect("gpu");
    let gpu_s = t0.elapsed().as_secs_f64();

    let t1 = std::time::Instant::now();
    let cpu = simplify_filter_cpu(&csr, &poss, &cert);
    let cpu_s = t1.elapsed().as_secs_f64();

    assert_eq!(cpu, gpu, "large case: GPU filter != CPU reference");
    eprintln!(
        "simplify-filter {n_rules} rules → {} kept: GPU {:.3}s ({:.1}M rules/s), CPU {:.3}s ({:.1}M rules/s), speedup {:.1}x",
        gpu.n_rules(),
        gpu_s,
        n_rules as f64 / gpu_s / 1e6,
        cpu_s,
        n_rules as f64 / cpu_s / 1e6,
        cpu_s / gpu_s,
    );
}
