mod astro_image;
mod calibration_masters;
mod common;
pub mod math;
mod stacking;
#[cfg(any(test, feature = "bench"))]
mod testing;

pub use astro_image::{
    AstroImage, AstroImageMetadata, BitPix, HotPixelMap, ImageDimensions, correct_hot_pixels,
};
pub use calibration_masters::CalibrationMasters;
pub use stacking::{FrameType, SigmaClipConfig, StackingMethod, stack_frames};

#[cfg(feature = "bench")]
pub mod bench {
    pub use crate::astro_image::demosaic::bench as demosaic;
    pub use crate::testing::{calibration_dir, first_raw_file};
}
