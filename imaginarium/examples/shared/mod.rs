//! What the examples share: the test image in, and the workspace's `test_output/imaginarium/`
//! out.

#![expect(
    clippy::print_stdout,
    reason = "an example reports what it did on stdout"
)]
use std::io::{BufReader, BufWriter, Write};

use common::fs;
use common::internals::output_path;
use imaginarium::{ColorFormat, InterleavedImage, RasterFormat};

pub(crate) fn load_lena_rgba_u8() -> InterleavedImage {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test_resources/lena_895x551.tiff"
    );
    let file = fs::open(path).expect("Failed to open lena.tiff");
    RasterFormat::Tiff
        .decode(BufReader::new(file), u64::MAX)
        .expect("Failed to load lena.tiff")
        .convert(ColorFormat::RGBA_U8)
}

pub(crate) fn save_image(image: &InterleavedImage, filename: &str) {
    let path = output_path(&format!("imaginarium/{filename}"));
    let format = RasterFormat::from_path(&path).expect("an output name with a known extension");
    let mut out = BufWriter::new(fs::create(&path).expect("Failed to create the output file"));
    format
        .encode(image, &mut out)
        .expect("Failed to save image");
    out.flush().expect("Failed to save image");
    println!("Saved: {}", path.display());
}

pub(crate) fn print_image_info(name: &str, image: &InterleavedImage) {
    println!(
        "{}: {}x{} {}",
        name,
        image.desc().size().width,
        image.desc().size().height,
        image.desc().color_format()
    );
}
