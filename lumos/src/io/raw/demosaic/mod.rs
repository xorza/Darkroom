//! Demosaicing module for CFA (Color Filter Array) sensors.
//!
//! This module provides demosaicing algorithms for different sensor types:
//! - Bayer CFA patterns (RGGB, BGGR, GRBG, GBRG)
//! - X-Trans 6x6 patterns (Fujifilm sensors)

pub(crate) mod bayer;
pub(crate) mod tiled;
pub(crate) mod xtrans;

/// What one decode costs: the bytes it leaves, and its peak on the way there, the output included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DemosaicMemory {
    pub(crate) output_bytes: usize,
    pub(crate) peak_bytes: usize,
}

impl DemosaicMemory {
    /// This decode inside a pass that at some point holds `bytes`: its peak raised to at least
    /// that.
    pub(crate) const fn with_peak_at_least(self, bytes: usize) -> Self {
        Self {
            output_bytes: self.output_bytes,
            peak_bytes: if bytes > self.peak_bytes {
                bytes
            } else {
                self.peak_bytes
            },
        }
    }
}

#[cfg(test)]
mod memory_tests {
    use crate::internals::cfa::XTRANS_PATTERN;
    use crate::io::image::cfa::CfaType;
    use crate::io::image::image_dimensions::ImageDimensions;
    use crate::io::raw::demosaic::bayer::{CfaPattern, rcd};
    use crate::io::raw::demosaic::xtrans::markesteijn;

    /// Mono holds its one plane. Both demosaics hold the input, the three output planes and the
    /// pool's tile buffers, whatever the frame's size.
    #[test]
    fn demosaic_memory_counts_each_plane_the_kernel_holds() {
        let even = ImageDimensions::new((10, 8), 1);
        let odd = ImageDimensions::new((5, 3), 1);

        let mono = CfaType::Mono.demosaic_memory(even);
        assert_eq!(mono.output_bytes, 80 * 4);
        assert_eq!(mono.peak_bytes, 80 * 4);

        for (cfa_type, workspace) in [
            (
                CfaType::Bayer(CfaPattern::Rggb),
                rcd::internals::workspace_bytes(),
            ),
            (
                CfaType::XTrans(XTRANS_PATTERN),
                markesteijn::internals::workspace_bytes(),
            ),
        ] {
            for dimensions in [even, odd] {
                let plane = dimensions.pixel_count() * 4;
                let memory = cfa_type.demosaic_memory(dimensions);
                assert_eq!(memory.output_bytes, 3 * plane, "{cfa_type:?}");
                assert_eq!(memory.peak_bytes, 4 * plane + workspace, "{cfa_type:?}");
            }
        }
    }
}
