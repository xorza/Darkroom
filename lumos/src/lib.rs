mod astro_image;
mod calibration_masters;
mod common;
pub mod math;
mod stacking;
mod star_detection;
#[cfg(any(test, feature = "bench"))]
mod testing;

pub use astro_image::{AstroImage, AstroImageMetadata, BitPix, HotPixelMap, ImageDimensions};
pub use calibration_masters::CalibrationMasters;
pub use stacking::{FrameType, ImageStack, MedianConfig, SigmaClipConfig, StackingMethod};
pub use star_detection::{Star, StarDetectionConfig, find_stars};

#[cfg(feature = "bench")]
pub mod bench {
    pub use crate::astro_image::demosaic::bench as demosaic;
    pub use crate::astro_image::hot_pixels::bench as hot_pixels;
    pub use crate::stacking::bench::{mean, median, sigma_clipped};
    pub use crate::star_detection::bench::{
        background, centroid, convolution, cosmic_ray, deblend, detection, median_filter,
    };
    pub use crate::testing::{calibration_dir, calibration_masters_dir, first_raw_file};
}
