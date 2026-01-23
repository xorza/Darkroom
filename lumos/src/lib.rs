mod astro_image;
mod calibration_masters;
mod stacking;
#[cfg(test)]
mod test_utils;

pub use astro_image::{AstroImage, AstroImageMetadata, BitPix, ImageDimensions};
pub use calibration_masters::CalibrationMasters;
pub use stacking::{FrameType, SigmaClipConfig, StackingMethod, stack_frames};
