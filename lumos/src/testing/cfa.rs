//! CFA frame builders for tests: a raw plane wrapped as a [`CfaImage`] of a given pattern.

use imaginarium::Buffer2;

use crate::io::image::cfa::{CfaImage, CfaType};
use crate::io::image::image_metadata::ImageMetadata;
use crate::math::size2us::Size2us;

/// The Fujifilm X-Trans colour pattern, as a 6×6 phase table of colour indices.
///
/// Shared by the defect-map tests and the cosmic-ray bench so one pattern describes X-Trans across
/// the fixtures rather than each restating the array.
pub(crate) const XTRANS_PATTERN: [[u8; 6]; 6] = [
    [1, 0, 1, 1, 2, 1],
    [2, 1, 2, 0, 1, 0],
    [1, 2, 1, 1, 0, 1],
    [1, 2, 1, 1, 0, 1],
    [0, 1, 0, 2, 1, 2],
    [1, 0, 1, 1, 2, 1],
];

/// A `CfaImage` over raw pixel data of `size` and `cfa_type`.
pub(crate) fn make_cfa(size: Size2us, pixels: Vec<f32>, cfa_type: CfaType) -> CfaImage {
    cfa_from_plane(Buffer2::new(size.width, size.height, pixels), cfa_type)
}

/// Wrap an already-built plane. The plane carries its own dimensions, so unlike [`make_cfa`] this
/// needs no `size` and copies nothing.
pub(crate) fn cfa_from_plane(data: Buffer2<f32>, cfa_type: CfaType) -> CfaImage {
    CfaImage {
        data,
        metadata: ImageMetadata {
            cfa_type: Some(cfa_type),
            ..Default::default()
        },
        quantization_sigma: None,
        nulls: None,
    }
}

/// Create a `CfaImage` filled with a constant value.
pub(crate) fn constant_cfa(size: Size2us, value: f32, cfa_type: CfaType) -> CfaImage {
    CfaImage {
        data: Buffer2::new_filled(size.width, size.height, value),
        metadata: ImageMetadata {
            cfa_type: Some(cfa_type),
            ..Default::default()
        },
        quantization_sigma: None,
        nulls: None,
    }
}
