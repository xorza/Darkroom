//! What the examples share: the test image in, and a directory under the system temporary
//! directory out, so a run never writes into the source tree.

#![expect(
    clippy::print_stdout,
    reason = "an example reports what it did on stdout"
)]
use std::env;
use std::fs;
use std::path::PathBuf;

use imaginarium::{ColorFormat, Image};

fn output_dir() -> PathBuf {
    env::temp_dir().join("imaginarium-examples")
}

pub(crate) fn load_lena_rgba_u8() -> Image {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test_resources/lena_895x551.tiff"
    );
    Image::read_file(path)
        .expect("Failed to load lena.tiff")
        .convert(ColorFormat::RGBA_U8)
}

pub(crate) fn ensure_output_dir() {
    fs::create_dir_all(output_dir()).expect("Failed to create output directory");
}

pub(crate) fn save_image(image: &Image, filename: &str) {
    let path = output_dir().join(filename);
    image.save_file(&path).expect("Failed to save image");
    println!("Saved: {}", path.display());
}

pub(crate) fn print_image_info(name: &str, image: &Image) {
    println!(
        "{}: {}x{} {}",
        name,
        image.desc().width,
        image.desc().height,
        image.desc().color_format
    );
}
