mod io;
mod stride;
mod tiff;

#[cfg(test)]
mod tests;

use std::path::Path;

use crate::common::conversion::convert_image;
use crate::common::{ColorFormat, Error, Result};

use stride::{add_stride_padding, align_stride, strip_stride_padding};

#[derive(Clone, Copy, Eq, PartialEq, Debug, Hash)]
pub struct ImageDesc {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub color_format: ColorFormat,
}

#[derive(Clone, Debug)]
pub struct Image {
    desc: ImageDesc,
    bytes: Vec<u8>,
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

    /// Returns the image bytes as a mutable slice.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }

    pub fn new_empty(desc: ImageDesc) -> Result<Image> {
        desc.color_format.validate()?;

        let bytes = vec![0; desc.size_in_bytes()];

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

        Ok(Image { desc, bytes })
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
            "tiff" | "tif" => io::save_tiff(self, filename)?,

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

        let mut result = Image::new_empty(desc)?;

        convert_image(&self, &mut result)?;

        Ok(result)
    }

    pub fn bytes_per_pixel(&self) -> u8 {
        self.desc.color_format.byte_count()
    }

    /// Returns an image with tightly packed pixel data (stride equals row bytes).
    pub fn packed(self) -> Image {
        let row_bytes = self.desc.width * self.desc.color_format.byte_count() as u32;
        if self.desc.stride == row_bytes {
            return self;
        }

        let bytes = strip_stride_padding(
            &self.bytes,
            self.desc.width,
            self.desc.height,
            self.desc.stride,
            self.desc.color_format.byte_count(),
        );

        Image {
            desc: ImageDesc {
                width: self.desc.width,
                height: self.desc.height,
                stride: row_bytes,
                color_format: self.desc.color_format,
            },
            bytes,
        }
    }

    /// Returns an image with 4-byte aligned stride padding applied.
    pub fn with_stride(self) -> Image {
        let aligned_stride =
            align_stride(self.desc.width * self.desc.color_format.byte_count() as u32);
        if self.desc.stride == aligned_stride {
            return self;
        }

        let bytes = add_stride_padding(
            &self.bytes,
            self.desc.width,
            self.desc.height,
            aligned_stride,
            self.desc.color_format.byte_count(),
        );

        Image {
            desc: ImageDesc {
                width: self.desc.width,
                height: self.desc.height,
                stride: aligned_stride,
                color_format: self.desc.color_format,
            },
            bytes,
        }
    }
}

impl ImageDesc {
    pub fn new(width: u32, height: u32, color_format: ColorFormat) -> Self {
        let stride = align_stride(
            width * color_format.channel_count as u32 * color_format.channel_size as u32,
        );

        Self {
            width,
            height,
            stride,
            color_format,
        }
    }

    pub fn size_in_bytes(&self) -> usize {
        (self.height * self.stride) as usize
    }

    /// Returns true if the stride is 4-byte aligned.
    pub fn is_aligned(&self) -> bool {
        self.stride.is_multiple_of(4)
    }

    /// Returns a new descriptor with 4-byte aligned stride.
    pub fn with_aligned_stride(self) -> Self {
        Self {
            stride: align_stride(self.width * self.color_format.byte_count() as u32),
            ..self
        }
    }
}
