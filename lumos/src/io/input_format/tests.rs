use std::path::Path;

use imaginarium::FileFormat;

use crate::io::error::ImageError;
use crate::io::input_format::{InputFormat, PREVIEW_IMAGE_EXTENSIONS, RAW_EXTENSIONS};

#[test]
fn the_extension_names_the_decoder_in_any_case() {
    for (path, expected) in [
        ("a.fits", InputFormat::Fits),
        ("a.FIT", InputFormat::Fits),
        ("dir.fits/a.Cr3", InputFormat::CameraRaw),
        ("a.raf", InputFormat::CameraRaw),
        ("a.TIF", InputFormat::Raster(FileFormat::Tiff)),
        ("a.jpeg", InputFormat::Raster(FileFormat::Jpeg)),
    ] {
        assert_eq!(
            InputFormat::of(Path::new(path)).unwrap(),
            expected,
            "{path}"
        );
    }
    for path in ["a.xyz", "no_extension", "fits"] {
        assert!(
            matches!(
                InputFormat::of(Path::new(path)),
                Err(ImageError::UnsupportedFormat { path: refused }) if refused == Path::new(path)
            ),
            "{path}"
        );
    }
}

/// FITS first, then every camera-RAW extension, then imaginarium's rasters; no extension names
/// two containers.
#[test]
fn preview_extensions_are_fits_then_raw_then_imaginarium() {
    let raw = RAW_EXTENSIONS;
    assert_eq!(&PREVIEW_IMAGE_EXTENSIONS[..2], ["fits", "fit"]);
    assert_eq!(&PREVIEW_IMAGE_EXTENSIONS[2..2 + raw.len()], raw);
    assert_eq!(
        &PREVIEW_IMAGE_EXTENSIONS[2 + raw.len()..],
        ["png", "jpg", "jpeg", "tiff", "tif"]
    );
    for extension in [
        "orf", "rw2", "pef", "nrw", "srw", "iiq", "3fr", "erf", "mrw", "rwl",
    ] {
        assert!(raw.contains(&extension), "{extension}");
    }
    let mut unique = PREVIEW_IMAGE_EXTENSIONS.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), PREVIEW_IMAGE_EXTENSIONS.len());
}
