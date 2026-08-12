pub(crate) mod cfa;
pub(crate) mod error;
pub(crate) mod fits;
pub(crate) mod image_dimensions;
pub(crate) mod image_metadata;
pub(crate) mod image_provenance;
pub(crate) mod linear;
pub(crate) mod linear_pixels;
pub(crate) mod load_context;
pub(crate) mod null_mask;
pub(crate) mod preview_image;
pub(crate) mod sample_domain;
pub(crate) mod sensor;
pub(crate) mod standard;

/// Every file extension accepted by [`preview_image::PreviewImage::from_file`]. Lives here rather
/// than beside the loader tables in [`standard`] because it is this module's published surface —
/// `lib.rs` re-exports it.
pub const PREVIEW_IMAGE_EXTENSIONS: &[&str] = &[
    "fits", "fit", "raf", "cr2", "cr3", "nef", "arw", "dng", "tiff", "tif", "png", "jpg", "jpeg",
];

#[cfg(test)]
mod tests;
