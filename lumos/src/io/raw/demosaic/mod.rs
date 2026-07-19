//! Demosaicing module for CFA (Color Filter Array) sensors.
//!
//! This module provides demosaicing algorithms for different sensor types:
//! - Bayer CFA patterns (RGGB, BGGR, GRBG, GBRG)
//! - X-Trans 6x6 patterns (Fujifilm sensors)
//!
//! The Bayer and X-Trans kernels share an explicit output-range policy.

pub(crate) mod bayer;
pub(crate) mod xtrans;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DemosaicRange {
    Unit,
    NonNegative,
    Preserve,
}

impl DemosaicRange {
    #[inline(always)]
    pub(crate) fn apply(self, value: f32) -> f32 {
        match self {
            Self::Unit => value.clamp(0.0, 1.0),
            Self::NonNegative => value.max(0.0),
            Self::Preserve => value,
        }
    }
}

/// Returned by a demosaic kernel when it observes the cancel token set
/// between stages. A marker only — the partial buffers are dropped; the caller
/// maps this to its own cancellation error.
#[derive(Debug)]
pub(crate) struct Cancelled;

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

#[cfg(test)]
mod tests {
    use crate::io::raw::demosaic::DemosaicRange;

    #[test]
    fn output_range_policies_are_distinct() {
        let values = [-0.25, 0.5, 1.25];

        assert_eq!(
            values.map(|value| DemosaicRange::Unit.apply(value)),
            [0.0, 0.5, 1.0]
        );
        assert_eq!(
            values.map(|value| DemosaicRange::NonNegative.apply(value)),
            [0.0, 0.5, 1.25]
        );
        assert_eq!(
            values.map(|value| DemosaicRange::Preserve.apply(value)),
            values
        );
    }
}
