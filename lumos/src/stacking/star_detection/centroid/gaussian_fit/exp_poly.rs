//! Cephes `exp()` polynomial coefficients, shared by the AVX2 and NEON Gaussian backends.
//!
//! Each backend implements the same approximation with its own intrinsics, so the coefficients
//! are the one part they genuinely share. Range reduction via `x = n·ln2 + r`, then the rational
//! approximation `exp(r) ≈ 1 + 2r·P(r²) / (Q(r²) − P(r²))`.
//!
//! Coefficients from the Cephes library (Stephen Moshier), public domain. Max relative error
//! < 2e-13.

pub(super) const EXP_P0: f64 = 1.261_771_930_748_105_8e-4;
pub(super) const EXP_P1: f64 = 3.029_944_077_074_419_5e-2;
pub(super) const EXP_P2: f64 = 1.0;

pub(super) const EXP_Q0: f64 = 3.001_985_051_386_644_6e-6;
pub(super) const EXP_Q1: f64 = 2.524_483_403_496_841e-3;
pub(super) const EXP_Q2: f64 = 2.272_655_482_081_550_3e-1;
pub(super) const EXP_Q3: f64 = 2.0;

/// ln(2) split into high and low parts for exact range reduction.
pub(super) const LN2_HI: f64 = 6.931_457_519_531_25e-1;
pub(super) const LN2_LO: f64 = 1.428_606_820_309_417_3e-6;
