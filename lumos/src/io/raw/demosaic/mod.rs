//! Demosaicing module for CFA (Color Filter Array) sensors.
//!
//! This module provides demosaicing algorithms for different sensor types:
//! - Bayer CFA patterns (RGGB, BGGR, GRBG, GBRG)
//! - X-Trans 6x6 patterns (Fujifilm sensors)

pub(crate) mod bayer;
pub(crate) mod xtrans;

/// Returned by a demosaic kernel when it observes the cancel token set
/// between stages. A marker only — the partial buffers are dropped; the caller
/// maps this to its own cancellation error.
#[derive(Debug)]
pub(crate) struct Cancelled;

#[derive(Debug, thiserror::Error)]
pub(crate) enum DemosaicError {
    #[error("demosaicing cancelled")]
    Cancelled,
    #[error(transparent)]
    InvalidXTransPattern(#[from] xtrans::XTransPatternError),
}

impl From<Cancelled> for DemosaicError {
    fn from(_: Cancelled) -> Self {
        Self::Cancelled
    }
}

/// Re-interleave planar `[R, G, B]` demosaic output to `[R0, G0, B0, R1, ...]`.
///
/// Test-only bridge: the demosaic kernels return planar channels, but their
/// tests assert against the original interleaved layout, so they re-interleave
/// the result and verify it matches the expected pixels.
#[cfg(test)]
pub(crate) fn interleave_planes(planes: [Vec<f32>; 3]) -> Vec<f32> {
    let [r, g, b] = planes;
    let mut out = vec![0.0f32; r.len() * 3];
    for (i, ((&rv, &gv), &bv)) in r.iter().zip(g.iter()).zip(b.iter()).enumerate() {
        out[i * 3] = rv;
        out[i * 3 + 1] = gv;
        out[i * 3 + 2] = bv;
    }
    out
}
