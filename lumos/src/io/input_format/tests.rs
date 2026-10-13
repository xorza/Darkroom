use std::path::Path;

use imaginarium::FileFormat;

use crate::io::error::ImageError;
use crate::io::input_format::InputFormat;

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
