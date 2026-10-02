//! Tests that read the calibration image set rather than synthetic pixels.
//!
//! Kept apart from the rest of `io::image`'s tests so the decoder-facing imports they need are
//! gated once, at the module, instead of one `cfg` per `use` in a file that is mostly feature-free.

use common::CancelToken;
use common::file_utils;

use common::internals::debug_output_path;

use crate::io::image::PREVIEW_IMAGE_EXTENSIONS;
use crate::io::image::cfa::CfaImage;
use crate::io::image::load_context::LoadContext;
use crate::io::image::standard::{FITS_EXTENSIONS, STANDARD_IMAGE_EXTENSIONS};
use crate::io::raw;

#[test]
fn loadable_extensions_match_decoder_policies() {
    let expected: Vec<&str> = FITS_EXTENSIONS
        .iter()
        .chain(raw::RAW_EXTENSIONS)
        .chain(STANDARD_IMAGE_EXTENSIONS)
        .copied()
        .collect();

    assert_eq!(PREVIEW_IMAGE_EXTENSIONS, expected);
}

#[test]
fn load_single_raw_from_env() {
    use crate::testing::init_tracing;
    use crate::testing::real_data::dataset_dir;

    init_tracing();

    let cal_dir = dataset_dir();

    let lights_dir = cal_dir.join("Lights");
    if !lights_dir.exists() {
        eprintln!("Lights directory not found, skipping test");
        return;
    }

    let files = file_utils::files_with_extensions(&lights_dir, raw::RAW_EXTENSIONS)
        .expect("scan RAW lights directory");
    let Some(first_file) = files.first() else {
        eprintln!("No image files in Lights, skipping test");
        return;
    };

    println!("Loading file: {first_file:?}");

    let image = CfaImage::from_file(first_file, &LoadContext::default())
        .expect("Failed to load CFA image")
        .demosaic(&CancelToken::never())
        .expect("Failed to demosaic CFA image");

    println!(
        "Loaded image: {}x{}x{}",
        image.width(),
        image.height(),
        image.channels()
    );
    println!("Mean: {}", image.mean());

    assert!(image.width() > 0);
    assert!(image.height() > 0);
    assert_eq!(image.channels(), 3);

    if let Some(path) = debug_output_path("light_from_raw.tiff") {
        imaginarium::Image::from(image).save_file(path).unwrap();
    }
}
