use tiff::encoder::colortype::ColorType;
use tiff::tags::{PhotometricInterpretation, SampleFormat};

pub struct GrayAlphaI8;
impl ColorType for GrayAlphaI8 {
    type Inner = i8;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[8, 8];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 2];
}
pub struct GrayAlphaI16;
impl ColorType for GrayAlphaI16 {
    type Inner = i16;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[16, 16];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 2];
}
pub struct GrayAlphaI32;
impl ColorType for GrayAlphaI32 {
    type Inner = i32;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[32, 32];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 2];
}
pub struct GrayAlphaI64;
impl ColorType for GrayAlphaI64 {
    type Inner = i64;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[64, 64];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 2];
}


pub struct GrayAlpha8;
impl ColorType for GrayAlpha8 {
    type Inner = u8;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[8, 8];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Uint; 2];
}
pub struct GrayAlpha16;
impl ColorType for GrayAlpha16 {
    type Inner = u16;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[16, 16];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Uint; 2];
}
pub struct GrayAlpha32;
impl ColorType for GrayAlpha32 {
    type Inner = u32;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[32, 32];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Uint; 2];
}
pub struct GrayAlpha64;
impl ColorType for GrayAlpha64 {
    type Inner = u64;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[64, 64];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Uint; 2];
}


pub struct GrayAlpha32Float;
impl ColorType for GrayAlpha32Float {
    type Inner = f32;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[32, 32];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::IEEEFP; 2];
}
pub struct GrayAlpha64Float;
impl ColorType for GrayAlpha64Float {
    type Inner = f64;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[64, 64];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::IEEEFP; 2];
}


pub struct RGBI8;
impl ColorType for RGBI8 {
    type Inner = i8;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::RGB;
    const BITS_PER_SAMPLE: &'static [u16] = &[8, 8, 8];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 3];
}
pub struct RGBI16;
impl ColorType for RGBI16 {
    type Inner = i16;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::RGB;
    const BITS_PER_SAMPLE: &'static [u16] = &[16, 16, 16];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 3];
}
pub struct RGBI32;
impl ColorType for RGBI32 {
    type Inner = i32;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::RGB;
    const BITS_PER_SAMPLE: &'static [u16] = &[32, 32, 32];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 3];
}
pub struct RGBI64;
impl ColorType for RGBI64 {
    type Inner = i64;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::RGB;
    const BITS_PER_SAMPLE: &'static [u16] = &[64, 64, 64];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 3];
}


pub struct RGBAI8;
impl ColorType for RGBAI8 {
    type Inner = i8;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::RGB;
    const BITS_PER_SAMPLE: &'static [u16] = &[8, 8, 8];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 4];
}
pub struct RGBAI16;
impl ColorType for RGBAI16 {
    type Inner = i16;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::RGB;
    const BITS_PER_SAMPLE: &'static [u16] = &[16, 16, 16];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 4];
}
pub struct RGBAI32;
impl ColorType for RGBAI32 {
    type Inner = i32;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::RGB;
    const BITS_PER_SAMPLE: &'static [u16] = &[32, 32, 32];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 4];
}
pub struct RGBAI64;
impl ColorType for RGBAI64 {
    type Inner = i64;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::RGB;
    const BITS_PER_SAMPLE: &'static [u16] = &[64, 64, 64];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 4];
}
