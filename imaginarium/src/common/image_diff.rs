//! Image comparison utilities for testing.

use rayon::prelude::*;

use crate::common::color_format::SampleType;
use crate::common::sample::Sample;
use crate::image::Image;

/// The largest per-channel difference between two images: normalized to `[0, 1]`
/// for the integer formats, absolute for float.
///
/// Both differences are taken in `f64`, so an `f32` pair never loses precision
/// to a rounded subtraction before the comparison.
///
/// # Panics
/// Panics unless the two images share a descriptor.
pub(crate) fn max_pixel_diff(img1: &Image, img2: &Image) -> f64 {
    img1.desc().assert_same(img2.desc(), "img1/img2");

    // Pixel data is tightly packed, so the two buffers are one flat channel
    // array each — there is no per-row padding to step over.
    let (a, b) = (img1.bytes(), img2.bytes());
    match img1.desc().color_format.sample_type {
        SampleType::U8 => max_diff::<u8>(a, b),
        SampleType::U16 => max_diff::<u16>(a, b),
        SampleType::F32 => max_diff::<f32>(a, b),
    }
}

/// The largest `|a - b|` over two buffers read as `T` channel values, over `T`'s full scale.
fn max_diff<T>(a: &[u8], b: &[u8]) -> f64
where
    T: Sample + Into<f64>,
{
    let scale = f64::from(T::FULL_SCALE_F32);
    let a: &[T] = bytemuck::cast_slice(a);
    let b: &[T] = bytemuck::cast_slice(b);
    a.par_iter()
        .zip(b.par_iter())
        .map(|(&a, &b)| (a.into() - b.into()).abs() / scale)
        .reduce(|| 0.0, f64::max)
}

/// Whether two images hold the same pixel data: identical bytes, except that any NaN matches
/// any NaN — Rust leaves the sign and payload of a NaN an operation produces unspecified, and a
/// vector kernel may combine two NaNs in the other operand order.
///
/// # Panics
/// Panics unless the two images share a descriptor.
pub(crate) fn pixels_equal(img1: &Image, img2: &Image) -> bool {
    img1.desc().assert_same(img2.desc(), "img1/img2");
    if !img1.desc().color_format.sample_type.is_float() {
        return img1.bytes() == img2.bytes();
    }
    let a: &[f32] = bytemuck::cast_slice(img1.bytes());
    let b: &[f32] = bytemuck::cast_slice(img2.bytes());
    a.iter()
        .zip(b)
        .all(|(a, b)| a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan()))
}

/// Asserts that a GPU result is the CPU reference to within what WGSL arithmetic allows.
///
/// The shaders evaluate the CPU's formulas in the same order, but WGSL lets a division err by
/// 2.5 ULP and lets a multiply-add fuse, so an `f32` result can differ by a few ULP — `1e-6` is
/// 8 ULP at one, the top of the range the ops produce. An integer result rounds such a value,
/// and one lying within those few ULP of a tie can land one step away.
#[cfg(feature = "wgpu")]
pub(crate) fn assert_matches_cpu(gpu: &Image, cpu: &Image, label: &str) {
    let tolerance = match cpu.desc().color_format.sample_type {
        SampleType::U8 => 1.0 / 255.0,
        SampleType::U16 => 1.0 / 65535.0,
        SampleType::F32 => 1e-6,
    };
    let difference = max_pixel_diff(gpu, cpu);
    assert!(
        difference <= tolerance,
        "{label}: GPU and CPU differ by {difference}, more than {tolerance}"
    );
}
