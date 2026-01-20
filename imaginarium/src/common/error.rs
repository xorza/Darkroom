use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    InvalidExtension(String),
    UnsupportedColorType(String),
    UnsupportedFormat(String),
    InvalidColorFormat(String),
    Conversion(String),
    Encoding(String),
    Gpu(String),
    NoGpuContext,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "IO error: {}", e),
            Error::InvalidExtension(ext) => write!(f, "Invalid file extension: {}", ext),
            Error::UnsupportedColorType(msg) => write!(f, "Unsupported color type: {}", msg),
            Error::UnsupportedFormat(msg) => write!(f, "Unsupported format: {}", msg),
            Error::InvalidColorFormat(msg) => write!(f, "Invalid color format: {}", msg),
            Error::Conversion(msg) => write!(f, "Conversion error: {}", msg),
            Error::Encoding(msg) => write!(f, "Encoding error: {}", msg),
            Error::Gpu(msg) => write!(f, "GPU error: {}", msg),
            Error::NoGpuContext => write!(f, "GPU context not available"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<image::ImageError> for Error {
    fn from(e: image::ImageError) -> Self {
        Error::Encoding(e.to_string())
    }
}

impl From<tiff::TiffError> for Error {
    fn from(e: tiff::TiffError) -> Self {
        Error::Encoding(e.to_string())
    }
}

impl From<bytemuck::PodCastError> for Error {
    fn from(e: bytemuck::PodCastError) -> Self {
        Error::Conversion(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
