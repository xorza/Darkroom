mod astro_image;
mod calibration_masters;
mod common;
pub mod math;
mod stacking;
#[cfg(any(test, feature = "bench"))]
mod testing;

pub use astro_image::{AstroImage, AstroImageMetadata, BitPix, HotPixelMap, ImageDimensions};
pub use calibration_masters::CalibrationMasters;
pub use stacking::{FrameType, ImageStack, MedianStackConfig, SigmaClipConfig, StackingMethod};

#[cfg(feature = "bench")]
pub mod bench {
    pub use crate::astro_image::demosaic::bench as demosaic;
    pub use crate::astro_image::hot_pixels::bench as hot_pixels;
    pub use crate::testing::{calibration_dir, calibration_masters_dir, first_raw_file};
}
