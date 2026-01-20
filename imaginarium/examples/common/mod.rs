#![allow(dead_code)]

use std::fs;
use std::path::Path;

use imaginarium::prelude::*;

/// Returns the workspace root directory (parent of the crate directory).
fn workspace_root() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/..")
}

fn output_dir() -> String {
    format!("{}/test_output/examples", workspace_root())
}

fn lena_path() -> String {
    format!("{}/test_resources/lena.tiff", workspace_root())
}

pub fn load_lena_rgba_u8() -> Image {
    Image::read_file(lena_path())
        .expect("Failed to load lena.tiff")
        .convert(ColorFormat::RGBA_U8)
        .expect("Failed to convert to RGBA_U8")
}

pub fn ensure_output_dir() {
    fs::create_dir_all(output_dir()).expect("Failed to create output directory");
}

fn output_path(filename: &str) -> String {
    Path::new(&output_dir())
        .join(filename)
        .to_string_lossy()
        .to_string()
}

pub fn save_image(image: &Image, filename: &str) {
    let path = output_path(filename);
    image.save_file(&path).expect("Failed to save image");
    println!("Saved: {}", path);
}

pub fn print_image_info(name: &str, image: &Image) {
    println!(
        "{}: {}x{} {}",
        name,
        image.desc().width,
        image.desc().height,
        image.desc().color_format
    );
}
