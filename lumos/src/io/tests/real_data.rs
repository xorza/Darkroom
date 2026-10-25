//! Tests that read the calibration image set rather than synthetic pixels.
//!
//! Kept apart from the rest of `io`'s tests so the decoder-facing imports they need are
//! gated once, at the module, instead of one `cfg` per `use` in a file that is mostly feature-free.

use common::internals;

use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::internals::real_data::raw_frames;
use crate::io::load_ctx::LoadCtx;
use crate::io::raster;

/// The first RAW light loads through the CFA entry point at the size its header declares, and
/// demosaics to three channels of that size.
#[test]
fn the_first_raw_light_loads_at_its_declared_size_and_demosaics() {
    let path = &raw_frames("Lights")[0];
    let ctx = LoadCtx::default();
    let declared = ctx.cfa_peek(path).unwrap().info.dimensions;
    let cfa = ctx.cfa(path).unwrap();
    assert_eq!(cfa.data().size(), declared.size());
    let image = cfa.demosaic_measured(MarkesteijnPasses::One);
    assert_eq!(image.dimensions().size(), declared.size());
    assert_eq!(image.channels(), 3);

    if let Some(path) = internals::debug_output_path("light_from_raw.tiff") {
        raster::write_image(&path, &imaginarium::Image::from(image)).unwrap();
    }
}
