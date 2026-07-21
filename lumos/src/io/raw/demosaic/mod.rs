//! Demosaicing module for CFA (Color Filter Array) sensors.
//!
//! This module provides demosaicing algorithms for different sensor types:
//! - Bayer CFA patterns (RGGB, BGGR, GRBG, GBRG)
//! - X-Trans 6x6 patterns (Fujifilm sensors)

pub(crate) mod bayer;
pub(crate) mod xtrans;

use std::mem::size_of;

use crate::io::image::ImageDimensions;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DemosaicMemory {
    pub(crate) output_bytes: usize,
    pub(crate) peak_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DemosaicKind {
    Mono,
    BayerRcd,
    XTransMarkesteijn,
}

impl DemosaicKind {
    pub(crate) fn memory(self, dimensions: ImageDimensions) -> DemosaicMemory {
        match self {
            Self::Mono => {
                let bytes = dimensions.pixel_count().saturating_mul(size_of::<f32>());
                DemosaicMemory {
                    output_bytes: bytes,
                    peak_bytes: bytes,
                }
            }
            Self::BayerRcd => bayer::rcd::demosaic_memory(
                dimensions.width(),
                dimensions.height(),
                dimensions.width(),
                dimensions.height(),
            ),
            Self::XTransMarkesteijn => {
                xtrans::markesteijn::demosaic_memory(dimensions.width(), dimensions.height())
            }
        }
    }
}

/// Returned by a demosaic kernel when it observes the cancel token set
/// between stages. A marker only — the partial buffers are dropped; the caller
/// maps this to its own cancellation error.
#[derive(Debug)]
pub(crate) struct Cancelled;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
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

#[cfg(test)]
mod memory_tests {
    use crate::io::image::ImageDimensions;
    use crate::io::raw::demosaic::DemosaicKind;

    #[test]
    fn demosaic_memory_matches_live_allocations() {
        let even = ImageDimensions::new((10, 8), 1);
        let odd = ImageDimensions::new((5, 3), 1);

        let mono = DemosaicKind::Mono.memory(even);
        assert_eq!(mono.output_bytes, 80 * 4);
        assert_eq!(mono.peak_bytes, 80 * 4);

        let bayer_even = DemosaicKind::BayerRcd.memory(even);
        assert_eq!(bayer_even.output_bytes, 3 * 80 * 4);
        assert_eq!(bayer_even.peak_bytes, 7 * 80 * 4);

        // RCD's two half-width diagonal buffers use ceil(width / 2), so an odd-width frame peaks
        // at 6P + 2 ceil(W/2)H = 6(15) + 2(3)(3) = 108 live f32 values.
        let bayer_odd = DemosaicKind::BayerRcd.memory(odd);
        assert_eq!(bayer_odd.output_bytes, 3 * 15 * 4);
        assert_eq!(bayer_odd.peak_bytes, 108 * 4);

        let xtrans = DemosaicKind::XTransMarkesteijn.memory(even);
        assert_eq!(xtrans.output_bytes, 3 * 80 * 4);
        assert_eq!(xtrans.peak_bytes, 22 * 80 * 4);
    }
}
