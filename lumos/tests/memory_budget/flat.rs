//! [`masters`]: a flat and the bias its lights' offset needs, so each light a run calibrates
//! carries the flat's gain.

use common::{Buffer2, CancelToken};
use lumos::{
    BayerPattern, CalibrationMasters, CalibrationSet, CfaImage, CfaType, ImageMetadata, Mosaic,
    SigmaThreshold,
};

/// The bias level, well below the sky every exposure sits on.
const BIAS: f32 = 0.002;

/// The masters of a `side` × `side` RGGB sensor: a flat, a vignetting from 1 at the centre down to
/// 0.8 in the corners, and a flat bias of [`BIAS`], which a light that holds an offset needs beside
/// a flat.
pub(crate) fn masters(side: usize) -> CalibrationMasters {
    let centre = (side as f32 - 1.0) / 2.0;
    let corner = 2.0 * centre * centre;
    let flat = Buffer2::from_fn((side, side), |x, y| {
        let (dx, dy) = (x as f32 - centre, y as f32 - centre);
        1.0 - 0.2 * (dx * dx + dy * dy) / corner
    });
    let mosaic = |plane| {
        CfaImage::from_plane(
            plane,
            CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb)),
            ImageMetadata::default(),
        )
    };
    CalibrationMasters::from_images(
        CalibrationSet {
            dark: None,
            flat: Some(mosaic(flat)),
            bias: Some(mosaic(Buffer2::new_filled((side, side), BIAS))),
            flat_dark: None,
        },
        SigmaThreshold::DEFAULT,
        &CancelToken::never(),
    )
    .unwrap()
}
