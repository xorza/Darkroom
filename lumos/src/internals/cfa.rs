//! CFA frame builders for tests: a raw plane wrapped as a [`CfaImage`] of a given pattern.

use imaginarium::Buffer2;

use crate::io::image::cfa::{CfaImage, CfaType};
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::raw::demosaic::xtrans::xtrans_pattern::XTransPattern;
use crate::math::size2us::Size2us;

/// A Fujifilm X-Trans colour layout: 8 red, 20 green and 8 blue, every green with as many red as
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
    cfa_from_plane(Buffer2::new(size.width, size.height, pixels), cfa_type)
}

/// Wrap an already-built plane. The plane carries its own dimensions, so unlike [`make_cfa`] this
/// needs no `size` and copies nothing.
pub(crate) fn cfa_from_plane(data: Buffer2<f32>, cfa_type: CfaType) -> CfaImage {
    CfaImage {
        data,
        cfa_type,
        metadata: ImageMetadata::default(),
        nulls: None,
    }
}

/// Create a `CfaImage` filled with a constant value.
pub(crate) fn constant_cfa(size: Size2us, value: f32, cfa_type: CfaType) -> CfaImage {
    cfa_from_plane(
        Buffer2::new_filled(size.width, size.height, value),
        cfa_type,
    )
}
