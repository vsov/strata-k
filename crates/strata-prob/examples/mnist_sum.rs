//! MNIST-sum learning demo — the Phase-4 exit. [neuro-symbolic parity]
//!
//! Trains a digit classifier from *sum labels only*, through the differentiable
//! provenance circuit, and prints the single-digit accuracy climbing from chance
//! to near-perfect — the DeepProbLog/Scallop result. Also shows the compilation
//! cache reuse (the sum circuits are built once, evaluated every example×epoch).
//!
//!   cargo run -p strata-prob --example mnist_sum --release

use strata_prob::mnist_sum::{train, Config};

fn main() {
    let cfg = Config::default();
    println!(
        "MNIST-sum: {} digits, {} training pairs, {} epochs — supervision is the SUM only.",
        cfg.digits, cfg.train_pairs, cfg.epochs
    );
    let r = train(&cfg);

    println!("single-digit test accuracy by epoch (0 = before training):");
    for (e, a) in r.accuracy.iter().enumerate() {
        let bar = "#".repeat((a * 40.0) as usize);
        println!("  {e:>2}: {:.3}  {bar}", a);
    }
    println!(
        "final digit accuracy: {:.1}%  (learned digits from sums alone — parity with DeepProbLog/Scallop)",
        100.0 * r.final_accuracy
    );
    println!(
        "compilation cache: {} circuits built once, reused {} times ({}× epochs × pairs)",
        r.cache_builds, r.cache_reuses, cfg.epochs
    );
    assert!(
        r.final_accuracy > 0.9,
        "expected digit-classification parity"
    );
    println!("OK — neuro-symbolic training converged via режим-B provenance + WMC gradients.");
}
