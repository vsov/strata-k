//! Provenance and the soft pipeline — режим B. [spec Phase 4]
//!
//! Probabilities are *not* weights: a naive semiring convolution double-counts a
//! fact shared by two derivations. режим B instead captures each tuple's
//! provenance and counts each fact once. The pipeline:
//!
//! - [`provenance`] — **chain capture**: derivations → an OR-of-ANDs circuit.
//! - [`circuit`] — the **SDD-class circuit** (decomposable AND, deterministic
//!   OR) with **weighted model counting** (exact marginal) and **gradients**
//!   (`∂P/∂p_i`, the bridge a neural layer trains through).
//! - [`topk`] — **top-k** proofs, the sparse differentiable surrogate when exact
//!   WMC is too costly.
//! - [`mnist_sum`] — the **exit**: learn digits from sum-only supervision
//!   (DeepProbLog/Scallop class), with a **compilation cache** reusing the
//!   circuits across epochs.

pub mod circuit;
pub mod mnist_sum;
pub mod provenance;
pub mod topk;

pub use circuit::{Builder, Circuit, Node};
pub use provenance::{build_dnf, sum_circuit};
