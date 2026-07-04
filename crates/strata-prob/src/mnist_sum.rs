//! MNIST-sum: the neuro-symbolic exit. [Phase 4]
//!
//! The canonical DeepProbLog / Scallop task: a neural net sees two images and
//! predicts a digit distribution for each; a logic program says their sum is the
//! only label. Trained end-to-end through the differentiable provenance circuit,
//! the net learns to classify *individual* digits from **sum supervision alone**
//! — the hallmark result. Here the "neural net" is a small linear classifier and
//! the images are perturbed one-hot features, so the learning signal is clean and
//! the mechanism (provenance → WMC → gradient) is what's on display.
//!
//! The pipeline it exercises: chain capture ([`crate::provenance::sum_circuit`]),
//! WMC + gradients ([`crate::circuit`]), a **compilation cache** (the 2·D−1 sum
//! circuits are built once and re-evaluated every epoch), and the gradient
//! interface a real autodiff framework (the "PyTorch bridge") would plug into.

use crate::circuit::Circuit;
use crate::provenance::sum_circuit;

/// Training configuration.
pub struct Config {
    pub digits: usize,
    pub train_pairs: usize,
    pub test_images: usize,
    pub epochs: usize,
    pub lr: f64,
    pub noise: f64,
    pub seed: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            digits: 10,
            train_pairs: 3000,
            test_images: 1000,
            epochs: 25,
            lr: 0.3,
            noise: 0.15,
            seed: 0xC0FFEE,
        }
    }
}

/// Outcome: per-epoch single-digit test accuracy, and cache reuse stats.
pub struct Report {
    pub accuracy: Vec<f64>,
    pub final_accuracy: f64,
    pub cache_builds: usize,
    pub cache_reuses: usize,
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn digit(&mut self, d: usize) -> usize {
        (self.next() as usize) % d
    }
}

/// A perturbed one-hot feature for digit `d` (feature dim = `digits`).
fn image(rng: &mut Rng, d: usize, digits: usize, noise: f64) -> Vec<f64> {
    (0..digits)
        .map(|i| if i == d { 1.0 } else { 0.0 } + noise * (rng.unit() - 0.5))
        .collect()
}

fn softmax(logits: &[f64]) -> Vec<f64> {
    let m = logits.iter().cloned().fold(f64::MIN, f64::max);
    let ex: Vec<f64> = logits.iter().map(|l| (l - m).exp()).collect();
    let z: f64 = ex.iter().sum();
    ex.iter().map(|e| e / z).collect()
}

/// logits = W·x + b for each digit.
fn forward(w: &[Vec<f64>], b: &[f64], x: &[f64]) -> Vec<f64> {
    w.iter()
        .zip(b)
        .map(|(row, bd)| row.iter().zip(x).map(|(wi, xi)| wi * xi).sum::<f64>() + bd)
        .collect()
}

/// dLoss/dlogits from dLoss/dp through the softmax with output `p`.
fn softmax_backward(p: &[f64], dp: &[f64]) -> Vec<f64> {
    let dot: f64 = p.iter().zip(dp).map(|(pi, di)| pi * di).sum();
    p.iter().zip(dp).map(|(pi, di)| pi * (di - dot)).collect()
}

fn accuracy(w: &[Vec<f64>], b: &[f64], test: &[(Vec<f64>, usize)]) -> f64 {
    let correct = test
        .iter()
        .filter(|(x, d)| {
            let logits = forward(w, b, x);
            let pred = logits
                .iter()
                .enumerate()
                .max_by(|a, z| a.1.partial_cmp(z.1).unwrap())
                .unwrap()
                .0;
            pred == *d
        })
        .count();
    correct as f64 / test.len() as f64
}

/// Train the digit classifier from sum labels only. Returns per-epoch accuracy.
pub fn train(cfg: &Config) -> Report {
    let d = cfg.digits;
    let mut rng = Rng(cfg.seed);

    // Compilation cache: every possible sum's circuit, built once.
    let cache: Vec<Circuit> = (0..(2 * d - 1)).map(|s| sum_circuit(s, d)).collect();
    let cache_builds = cache.len();
    let mut cache_reuses = 0usize;

    // Model: W [d][d], bias [d], small random init.
    let mut w: Vec<Vec<f64>> = (0..d)
        .map(|_| (0..d).map(|_| 0.01 * (rng.unit() - 0.5)).collect())
        .collect();
    let mut b: Vec<f64> = vec![0.0; d];

    // Data: training pairs (feature1, digit1, feature2, digit2), test singletons.
    let train: Vec<(Vec<f64>, usize, Vec<f64>, usize)> = (0..cfg.train_pairs)
        .map(|_| {
            let (d1, d2) = (rng.digit(d), rng.digit(d));
            (
                image(&mut rng, d1, d, cfg.noise),
                d1,
                image(&mut rng, d2, d, cfg.noise),
                d2,
            )
        })
        .collect();
    let test: Vec<(Vec<f64>, usize)> = (0..cfg.test_images)
        .map(|_| {
            let dd = rng.digit(d);
            (image(&mut rng, dd, d, cfg.noise), dd)
        })
        .collect();

    // Record accuracy before any training (≈ chance, 1/digits).
    let mut acc = vec![accuracy(&w, &b, &test)];
    let eps = 1e-9;
    for _epoch in 0..cfg.epochs {
        for (x1, d1, x2, d2) in &train {
            let p1 = softmax(&forward(&w, &b, x1));
            let p2 = softmax(&forward(&w, &b, x2));
            let leaves: Vec<f64> = p1.iter().chain(&p2).copied().collect();

            let s = d1 + d2;
            cache_reuses += 1;
            let (wmc, lg) = cache[s].grad(&leaves);

            // Loss = -log P(sum = s); backprop to leaf probabilities.
            let dl_dwmc = -1.0 / (wmc + eps);
            let dp1: Vec<f64> = (0..d).map(|i| dl_dwmc * lg[i]).collect();
            let dp2: Vec<f64> = (0..d).map(|i| dl_dwmc * lg[d + i]).collect();

            // Through the softmax, then into W, b (online SGD).
            let dz1 = softmax_backward(&p1, &dp1);
            let dz2 = softmax_backward(&p2, &dp2);
            for k in 0..d {
                for f in 0..d {
                    w[k][f] -= cfg.lr * (dz1[k] * x1[f] + dz2[k] * x2[f]);
                }
                b[k] -= cfg.lr * (dz1[k] + dz2[k]);
            }
        }
        acc.push(accuracy(&w, &b, &test));
    }

    let final_accuracy = *acc.last().unwrap_or(&0.0);
    Report {
        accuracy: acc,
        final_accuracy,
        cache_builds,
        cache_reuses,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learns_digits_from_sums() {
        let cfg = Config::default();
        let r = train(&cfg);
        // Learned to classify single digits from sum-only supervision — parity
        // with DeepProbLog/Scallop (which report ~97%+ on real MNIST-sum).
        assert!(
            r.final_accuracy > 0.9,
            "final digit accuracy {} should exceed 0.9; curve {:?}",
            r.final_accuracy,
            r.accuracy
        );
        // Accuracy improved over training (started near chance).
        assert!(r.accuracy[0] < r.final_accuracy);
        // Compilation cache: 19 circuits built once, reused every example×epoch.
        assert_eq!(r.cache_builds, 2 * cfg.digits - 1);
        assert_eq!(r.cache_reuses, cfg.epochs * cfg.train_pairs);
    }
}
