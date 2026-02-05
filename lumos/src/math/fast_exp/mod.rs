//! Vectorized fast exponential (expf) approximation using SIMD.
//!
//! Provides high-accuracy exp(x) for batches of f32 values using AVX2 or SSE,
//! processing 8 or 4 values simultaneously without leaving the SIMD pipeline.
//!
//! # Accuracy
//! Maximum relative error < 2 ULP (~2.4e-7) for the full f32 input range.
//! This is suitable for Levenberg-Marquardt optimization where convergence
//! depends on consistent, accurate Jacobian evaluation.
//!
//! # Algorithm
//! Cephes-style range reduction + polynomial approximation:
//! 1. Compute n = round(x / ln2), so x = n·ln2 + r where |r| ≤ ln2/2
//! 2. Evaluate exp(r) ≈ 1 + r·P(r) using a degree-5 minimax polynomial
//! 3. Reconstruct exp(x) = 2^n · exp(r) by adding n to the IEEE 754 exponent

use common::{cfg_aarch64, cfg_x86_64};

cfg_x86_64! {
    pub(crate) mod avx2;
    pub(crate) mod sse;
}

cfg_aarch64! {
    pub(crate) mod neon;

    pub use neon::fast_exp_4_neon;
}

// Cephes expf polynomial coefficients (minimax on [-ln2/2, ln2/2])
// exp(r) ≈ 1 + r + c2·r² + c3·r³ + c4·r⁴ + c5·r⁵ + c6·r⁶
pub(crate) const EXP_C1: f32 = 1.0;
pub(crate) const EXP_C2: f32 = 0.5; // 1/2!
pub(crate) const EXP_C3: f32 = 0.166_666_67; // 1/3!
pub(crate) const EXP_C4: f32 = 0.041_666_668; // 1/4!
pub(crate) const EXP_C5: f32 = 0.008_333_334; // 1/5!
pub(crate) const EXP_C6: f32 = 0.001_388_889_1; // 1/6!

pub(crate) const LOG2E: f32 = std::f32::consts::LOG2_E; // 1.442695...

// Split ln2 into high and low parts for exact range reduction
// ln2 = LN2_HI + LN2_LO with LN2_HI having fewer significant bits
pub(crate) const LN2_HI: f32 = 0.693_145_75; // upper bits
pub(crate) const LN2_LO: f32 = 1.428_606_8e-6; // lower bits (ln2 - LN2_HI)

/// Clamp bounds to avoid overflow/underflow in f32 exp
pub(crate) const EXP_LO: f32 = -87.3; // exp(-87.3) ≈ smallest normal f32
pub(crate) const EXP_HI: f32 = 88.0; // 88/ln2 ≈ 126.9, keeps 2^n within f32 range

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;
