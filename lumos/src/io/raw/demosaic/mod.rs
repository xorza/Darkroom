//! Demosaicing module for CFA (Color Filter Array) sensors.
//!
//! This module provides demosaicing algorithms for different sensor types:
//! - Bayer CFA patterns (RGGB, BGGR, GRBG, GBRG)
//! - X-Trans 6x6 patterns (Fujifilm sensors)

pub(crate) mod bayer;
pub(crate) mod tiled;
pub(crate) mod xtrans;

#[cfg(test)]
mod memory_tests {
    use crate::internals::cfa::XTRANS_PATTERN;
    use crate::io::image::cfa::CfaType;
    use crate::io::image::cfa::cfa_pattern::CfaPattern;
    use crate::io::image::image_dimensions::ImageDimensions;
    use crate::io::raw::demosaic::bayer::rcd;
    use crate::io::raw::demosaic::xtrans::markesteijn;
    use crate::memory::footprint::Footprint;

    /// Mono holds its one plane. Both demosaics hold the input, the three output planes and the
    /// tile buffers of each of the run's workers, whatever the frame's size: four workers' here.
    #[test]
    fn demosaic_footprint_counts_each_plane_the_kernel_holds() {
        let even = ImageDimensions::new((10, 8), 1);
        let odd = ImageDimensions::new((5, 3), 1);

        let mono = CfaType::Mono.demosaic_footprint(even, 4);
        assert_eq!(mono, Footprint::kept(80 * 4));

        for (cfa_type, workspace) in [
            (
                CfaType::Bayer(CfaPattern::Rggb),
                rcd::internals::workspace_bytes(4),
            ),
            (
                CfaType::XTrans(XTRANS_PATTERN),
                markesteijn::internals::workspace_bytes(4),
            ),
        ] {
            for dimensions in [even, odd] {
                let plane = dimensions.pixel_count() * 4;
                assert_eq!(
                    cfa_type.demosaic_footprint(dimensions, 4),
                    Footprint {
                        kept: 3 * plane,
                        peak: 4 * plane + workspace,
                    },
                    "{cfa_type:?}"
                );
            }
        }
    }
}
