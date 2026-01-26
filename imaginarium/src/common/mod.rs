pub(crate) mod color;
pub(crate) mod color_format;
pub(crate) mod conversion;
pub(crate) mod error;
pub(crate) mod image_diff;
#[cfg(test)]
pub(crate) mod test_utils;

// Public API
pub use color::Color;
pub use color_format::{
    ALL_FORMATS, ALPHA_FORMATS, ChannelCount, ChannelSize, ChannelType, ColorFormat,
};
pub use error::{Error, Result};
