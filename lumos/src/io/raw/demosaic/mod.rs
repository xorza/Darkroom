//! Demosaicing module for CFA (Color Filter Array) sensors.
//!
//! This module provides demosaicing algorithms for different sensor types:
//! - Bayer CFA patterns (RGGB, BGGR, GRBG, GBRG)
//! - X-Trans 6x6 patterns (Fujifilm sensors)

pub(crate) mod bayer;
pub(crate) mod sensor_layout;
pub(crate) mod xtrans;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DemosaicMemory {
    pub(crate) output_bytes: usize,
    pub(crate) peak_bytes: usize,
}

#[cfg(test)]
mod memory_tests {
    use crate::io::image::cfa::CfaType;
    use crate::io::image::image_dimensions::ImageDimensions;
    use crate::io::raw::demosaic::bayer::CfaPattern;
    use crate::testing::cfa::XTRANS_PATTERN;

    #[test]
    fn demosaic_memory_counts_each_plane_the_kernel_holds() {
        let even = ImageDimensions::new((10, 8), 1);
        let odd = ImageDimensions::new((5, 3), 1);

        let mono = CfaType::Mono.demosaic_memory(even);
        assert_eq!(mono.output_bytes, 80 * 4);
        assert_eq!(mono.peak_bytes, 80 * 4);

        let bayer = CfaType::Bayer(CfaPattern::Rggb);
        let bayer_even = bayer.demosaic_memory(even);
        assert_eq!(bayer_even.output_bytes, 3 * 80 * 4);
        assert_eq!(bayer_even.peak_bytes, 7 * 80 * 4);

        // RCD's two half-width diagonal buffers use ceil(width / 2), so an odd-width frame peaks
        // at 6P + 2 ceil(W/2)H = 6(15) + 2(3)(3) = 108 live f32 values.
        let bayer_odd = bayer.demosaic_memory(odd);
        assert_eq!(bayer_odd.output_bytes, 3 * 15 * 4);
        assert_eq!(bayer_odd.peak_bytes, 108 * 4);

        let xtrans = CfaType::XTrans(XTRANS_PATTERN).demosaic_memory(even);
        assert_eq!(xtrans.output_bytes, 3 * 80 * 4);
        assert_eq!(xtrans.peak_bytes, 22 * 80 * 4);
    }
}
