pub(crate) mod aligned_bytes;
pub(crate) mod color_format;
pub(crate) mod conversion;
pub(crate) mod error;
pub(crate) mod image_diff;
#[cfg(test)]
pub(crate) mod test_utils;

// Public API
pub use aligned_bytes::AlignedBytes;
pub use color_format::{
    ALL_FORMATS, ALPHA_FORMATS, ChannelCount, ChannelSize, ChannelType, ColorFormat,
};
pub use error::{Error, Result};
