mod astro_image;
mod stacking;

pub use astro_image::{AstroImage, AstroImageMetadata, ImageDimensions};
pub use stacking::{SigmaClipConfig, StackingMethod, stack_darks};
