//! CFA frame builders for tests: a raw plane wrapped as a [`CfaImage`] of a given pattern.

use common::Buffer2;

use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::xtrans_pattern::XTransPattern;
use crate::image::metadata::image_metadata::ImageMetadata;
use common::Size2us;

/// A Fujifilm X-Trans color layout: 8 red, 20 green and 8 blue, every green with as many red as
/// blue neighbours.
///
/// The one X-Trans fixture, shared by the demosaic, defect-map and cosmic-ray tests and benches.
pub(crate) const XTRANS_PATTERN: XTransPattern = match XTransPattern::new([
    [1, 1, 0, 1, 1, 2],
    [1, 1, 2, 1, 1, 0],
    [2, 0, 1, 0, 2, 1],
    [1, 1, 2, 1, 1, 0],
    [1, 1, 0, 1, 1, 2],
    [0, 2, 1, 2, 0, 1],
]) {
    Ok(pattern) => pattern,
    Err(_) => panic!("the X-Trans fixture is a valid layout"),
};

/// A `CfaImage` over raw pixel data of `size` and `cfa_type`.
pub(crate) fn make_cfa(size: Size2us, pixels: Vec<f32>, cfa_type: CfaType) -> CfaImage {
    cfa_from_plane(Buffer2::new(size, pixels), cfa_type)
}

/// Wrap an already-built plane. The plane carries its own dimensions, so unlike [`make_cfa`] this
/// needs no `size` and copies nothing.
pub(crate) fn cfa_from_plane(data: Buffer2<f32>, cfa_type: CfaType) -> CfaImage {
    CfaImage::from_plane(data, cfa_type, ImageMetadata::default())
}

/// Create a `CfaImage` filled with a constant value.
pub(crate) fn constant_cfa(size: Size2us, value: f32, cfa_type: CfaType) -> CfaImage {
    cfa_from_plane(Buffer2::new_filled(size, value), cfa_type)
}
