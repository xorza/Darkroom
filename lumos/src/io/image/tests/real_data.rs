//! Tests that read the calibration image set rather than synthetic pixels.
//!
//! Kept apart from the rest of `io::image`'s tests so the decoder-facing imports they need are
//! gated once, at the module, instead of one `cfg` per `use` in a file that is mostly feature-free.

use common::CancelToken;
use common::internals::debug_output_path;

use crate::io::image::cfa::{CfaFrameInfo, CfaImage};
use crate::io::image::load_context::LoadContext;
use crate::testing::real_data::raw_frames;

/// The first RAW light loads through the CFA entry point at the size its header declares, and
/// demosaics to three channels of that size.
#[test]
fn the_first_raw_light_loads_at_its_declared_size_and_demosaics() {
    let path = &raw_frames("Lights")[0];
    let context = LoadContext::default();
    let declared = CfaFrameInfo::from_file(path, &context).unwrap().dimensions;
    let cfa = CfaImage::from_file(path, &context).unwrap();
    assert_eq!(
        (cfa.data.width(), cfa.data.height()),
        (declared.width(), declared.height())
    );
    let image = cfa.demosaic(&CancelToken::never()).unwrap();
    assert_eq!(image.dimensions().size(), declared.size());
    assert_eq!(image.channels(), 3);

    if let Some(path) = debug_output_path("light_from_raw.tiff") {
        imaginarium::Image::from(image).save_file(path).unwrap();
    }
}
