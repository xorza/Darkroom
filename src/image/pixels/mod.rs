//! Typed interleaved and planar pixel layouts plus `Image`'s format-erased storage.

pub(crate) mod image_pixels;
pub(crate) mod interleaved;
pub(crate) mod planar;

#[cfg(test)]
mod tests;
