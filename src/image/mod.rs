mod conversion;
mod io;
pub(crate) mod pixels;
mod tiff;
mod transpose;

#[cfg(test)]
mod tests;

use std::path::Path;

/// Supported image file extensions for reading and writing.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "tiff", "tif"];

use crate::common::color_format::ColorFormat;
use crate::common::error::{Error, Result};
use crate::image::conversion::convert_image;
use crate::image::pixels::image_pixels::ImagePixels;

/// Image dimensions + pixel format. Pixel data is **always tightly packed**
/// (`row_bytes == width * bytes_per_pixel`, no inter-row padding) — any row
/// alignment a GPU backend needs lives inside `GpuImage` (`src/gpu/`), never here.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Hash)]
pub struct ImageDesc {
    pub width: usize,
    pub height: usize,
    pub color_format: ColorFormat,
}

/// A runtime-format image backed by tightly packed, typed interleaved pixels.
#[derive(Clone, Debug)]
pub struct Image {
    pixels: ImagePixels,
}

impl Image {
    /// Dimensions and format derived from the owned typed storage.
    #[inline]
    pub fn desc(&self) -> ImageDesc {
        self.pixels.desc()
    }

    /// The interleaved pixel bytes — a zero-copy `&[u8]` view of the typed buffer.
    pub fn bytes(&self) -> &[u8] {
        self.pixels.bytes()
    }

    /// Copy the pixel bytes into an owned `Vec<u8>` (the typed buffer's allocation
    /// can't be reinterpreted as a `Vec<u8>`, so this copies).
    pub fn into_bytes(self) -> Vec<u8> {
        self.pixels.bytes().to_vec()
    }

    /// The interleaved pixel bytes, mutable — zero-copy; writes hit the buffer.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        self.pixels.bytes_mut()
    }

    pub fn new_black(desc: ImageDesc) -> Result<Image> {
        desc.validate()?;
        let pixels = ImagePixels::new_zeroed(desc.color_format, desc.width, desc.height);
        Ok(Image { pixels })
    }

    pub fn new_with_data(desc: ImageDesc, bytes: Vec<u8>) -> Result<Image> {
        desc.validate()?;

        if bytes.len() != desc.size_in_bytes() {
            return Err(Error::SizeMismatch(format!(
                "bytes length {} does not match expected size {}",
                bytes.len(),
                desc.size_in_bytes()
            )));
        }

        let pixels = ImagePixels::from_bytes(desc.color_format, desc.width, desc.height, &bytes);
        Ok(Image { pixels })
    }

    pub fn read_file<P: AsRef<Path>>(filename: P) -> Result<Image> {
        let extension = filename
            .as_ref()
            .extension()
            .and_then(|os_str| os_str.to_str())
            .ok_or_else(|| Error::InvalidExtension("missing extension".to_string()))?
            .to_ascii_lowercase();

        let image = match extension.as_str() {
            "png" | "jpeg" | "jpg" => io::load_png_jpeg(filename)?,
            "tiff" | "tif" => io::load_tiff(filename)?,

            _ => return Err(Error::InvalidExtension(extension)),
        };

        Ok(image)
    }

    pub fn save_file<P: AsRef<Path>>(&self, filename: P) -> Result<()> {
        let extension = filename
            .as_ref()
            .extension()
            .and_then(|os_str| os_str.to_str())
            .ok_or_else(|| Error::InvalidExtension("missing extension".to_string()))?
            .to_ascii_lowercase();

        match extension.as_str() {
            "png" => io::save_png(self, filename)?,
            "jpeg" | "jpg" => io::save_jpg(self, filename)?,
            "tiff" | "tif" => tiff::save_tiff(self, filename)?,

            _ => return Err(Error::InvalidExtension(extension)),
        };

        Ok(())
    }

    pub fn convert(self, color_format: ColorFormat) -> Result<Image> {
        if self.desc().color_format == color_format {
            color_format.validate()?;
            return Ok(self);
        }
        self.convert_to(color_format)
    }

    /// Borrowing counterpart of [`convert`](Self::convert): converts into a freshly
    /// allocated image, leaving `self` alone — a caller that only holds a view (e.g.
    /// a CPU borrow of an `ImageBuffer`) skips the source deep-copy that `convert`'s
    /// `self` receiver would force. Same-format is a valid (if pointless) full copy.
    pub fn convert_to(&self, color_format: ColorFormat) -> Result<Image> {
        color_format.validate()?;

        let source_desc = self.desc();
        let desc = ImageDesc::new(source_desc.width, source_desc.height, color_format);
        let mut result = Image::new_black(desc)?;

        convert_image(self, &mut result);

        Ok(result)
    }

    pub fn bytes_per_pixel(&self) -> u8 {
        self.desc().color_format.byte_count()
    }
}

impl ImageDesc {
    /// Create a new (tightly packed) image descriptor.
    pub fn new(width: usize, height: usize, color_format: ColorFormat) -> Self {
        Self {
            width,
            height,
            color_format,
        }
    }

    /// Total packed byte size: `height * row_bytes`.
    pub fn size_in_bytes(&self) -> usize {
        self.height * self.row_bytes()
    }

    /// Bytes per (packed) row: `width * bytes_per_pixel`.
    pub fn row_bytes(&self) -> usize {
        self.width * self.color_format.byte_count() as usize
    }

    /// Validates the descriptor: positive dimensions, valid format.
    pub fn validate(&self) -> Result<()> {
        self.color_format.validate()?;
        if self.width == 0 || self.height == 0 {
            return Err(Error::SizeMismatch(format!(
                "image dimensions must be non-zero, got {}x{}",
                self.width, self.height
            )));
        }
        Ok(())
    }
}

impl std::fmt::Display for ImageDesc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{} {}", self.width, self.height, self.color_format)
    }
}
