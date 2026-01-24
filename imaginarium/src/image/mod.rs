mod io;
mod stride;
mod tiff;

#[cfg(test)]
mod tests;

use std::path::Path;

use aligned_vec::AVec;

/// Supported image file extensions for reading and writing.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "tiff", "tif"];

use crate::common::conversion::convert_image;
use crate::common::{ColorFormat, Error, Result};

use stride::{add_stride_padding, align_stride, strip_stride_padding};

/// 8-byte alignment for image data to allow zero-copy casting to f32/f64.
const ALIGNMENT: usize = 8;

#[derive(Clone, Copy, Eq, PartialEq, Debug, Hash)]
pub struct ImageDesc {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub color_format: ColorFormat,
}

/// An image with pixel data stored in 8-byte aligned memory.
///
/// The 8-byte alignment allows zero-copy casting to/from `Vec<f32>` or `Vec<f64>`.
#[derive(Clone, Debug)]
pub struct Image {
    desc: ImageDesc,
    bytes: AVec<u8>,
}

impl Image {
    /// Returns the image descriptor.
    pub fn desc(&self) -> &ImageDesc {
        &self.desc
    }

    /// Returns the image bytes as a slice.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Convert to owned bytes (zero-copy due to 8-byte alignment).
    pub fn into_bytes(self) -> Vec<u8> {
        let (ptr, _align, len, capacity) = self.bytes.into_raw_parts();
        // Safety: AVec guarantees the pointer is valid and properly aligned.
        unsafe { Vec::from_raw_parts(ptr, len, capacity) }
    }

    /// Convert to owned aligned bytes (internal use).
    fn into_aligned_bytes(self) -> AVec<u8> {
        self.bytes
    }

    /// Returns the image bytes as a mutable slice.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }

    pub fn new_black(desc: ImageDesc) -> Result<Image> {
        desc.color_format.validate()?;

        let mut bytes = AVec::with_capacity(ALIGNMENT, desc.size_in_bytes());
        bytes.resize(desc.size_in_bytes(), 0);

        Ok(Image { desc, bytes })
    }

    pub fn new_with_data(desc: ImageDesc, bytes: Vec<u8>) -> Result<Image> {
        desc.color_format.validate()?;

        if bytes.len() != desc.size_in_bytes() {
            return Err(Error::InvalidColorFormat(format!(
                "bytes length {} does not match expected size {}",
                bytes.len(),
                desc.size_in_bytes()
            )));
        }

        Ok(Image {
            desc,
            bytes: vec_to_avec(bytes),
        })
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

        // Strip stride padding if present (all formats expect tightly packed pixels)
        let image = if self.desc.is_packed() {
            std::borrow::Cow::Borrowed(self)
        } else {
            std::borrow::Cow::Owned(self.clone().packed())
        };

        match extension.as_str() {
            "png" => io::save_png(&image, filename)?,
            "jpeg" | "jpg" => io::save_jpg(&image, filename)?,
            "tiff" | "tif" => io::save_tiff(&image, filename)?,

            _ => return Err(Error::InvalidExtension(extension)),
        };

        Ok(())
    }

    pub fn convert(self, color_format: ColorFormat) -> Result<Image> {
        color_format.validate()?;

        if self.desc.color_format == color_format {
            return Ok(self);
        }

        let desc = ImageDesc::new(self.desc.width, self.desc.height, color_format);

        let mut result = Image::new_black(desc)?;

        convert_image(&self, &mut result)?;

        Ok(result)
    }

    pub fn bytes_per_pixel(&self) -> u8 {
        self.desc.color_format.byte_count()
    }

    /// Returns an image with tightly packed pixel data (stride equals row bytes).
    pub fn packed(self) -> Image {
        if self.desc.is_packed() {
            return self;
        }
        let desc = *self.desc();
        let bytes = strip_stride_padding(
            self.into_aligned_bytes(),
            desc.width,
            desc.height,
            desc.stride,
            desc.color_format.byte_count(),
        );

        Image {
            desc: ImageDesc {
                width: desc.width,
                height: desc.height,
                stride: desc.row_bytes(),
                color_format: desc.color_format,
            },
            bytes,
        }
    }

    /// Returns an image with 4-byte aligned stride padding applied.
    pub fn with_stride(self) -> Image {
        let aligned_stride = align_stride(self.desc.row_bytes());
        if self.desc.stride == aligned_stride {
            return self;
        }

        let desc = *self.desc();
        let bytes = add_stride_padding(
            self.into_aligned_bytes(),
            desc.width,
            desc.height,
            aligned_stride,
            desc.color_format.byte_count(),
        );

        Image {
            desc: ImageDesc {
                width: desc.width,
                height: desc.height,
                stride: aligned_stride,
                color_format: desc.color_format,
            },
            bytes,
        }
    }
}

/// Convert Vec<u8> to AVec<u8>, zero-copy if already aligned, otherwise copies.
fn vec_to_avec(bytes: Vec<u8>) -> AVec<u8> {
    let ptr = bytes.as_ptr();
    if (ptr as usize).is_multiple_of(ALIGNMENT) {
        // Already aligned - zero-copy conversion
        let (ptr, len, capacity) = {
            let mut bytes = std::mem::ManuallyDrop::new(bytes);
            (bytes.as_mut_ptr(), bytes.len(), bytes.capacity())
        };
        // Safety: pointer is verified to be ALIGNMENT-aligned, and we own the memory
        unsafe { AVec::from_raw_parts(ptr, ALIGNMENT, len, capacity) }
    } else {
        // Not aligned - must copy
        AVec::from_slice(ALIGNMENT, &bytes)
    }
}

impl ImageDesc {
    pub fn new(width: usize, height: usize, color_format: ColorFormat) -> Self {
        let stride = align_stride(
            width * color_format.channel_count as usize * color_format.channel_size as usize,
        );

        Self {
            width,
            height,
            stride,
            color_format,
        }
    }

    pub fn size_in_bytes(&self) -> usize {
        self.height * self.stride
    }

    /// Returns the number of bytes per row without padding.
    pub fn row_bytes(&self) -> usize {
        self.width * self.color_format.byte_count() as usize
    }

    /// Returns true if stride equals row bytes (no padding).
    pub fn is_packed(&self) -> bool {
        self.stride == self.row_bytes()
    }

    /// Returns true if the stride is 4-byte aligned.
    pub fn is_aligned(&self) -> bool {
        self.stride.is_multiple_of(4)
    }

    /// Returns a new descriptor with 4-byte aligned stride.
    pub fn with_aligned_stride(self) -> Self {
        Self {
            stride: align_stride(self.row_bytes()),
            ..self
        }
    }
}

impl std::fmt::Display for ImageDesc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{} {}", self.width, self.height, self.color_format)
    }
}
