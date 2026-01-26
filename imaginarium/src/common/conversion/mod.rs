mod conversion_scalar;
mod conversion_simd;

#[cfg(test)]
mod tests;

#[cfg(feature = "bench")]
pub mod bench;

pub(crate) use conversion_scalar::ChannelConvert;

use crate::common::error::Result;
use crate::image::Image;

// =============================================================================
// Public conversion extension trait
// =============================================================================

/// Extension trait providing channel conversion methods.
///
/// This trait is automatically implemented for all types that support
/// channel conversion and provides convenient `convert()` and `convert_to()`
/// methods.
pub trait Convert<To>: Sized {
    /// Convert this value to the target type.
    fn convert(self) -> To;
}

/// Extension trait for explicit type conversion using turbofish syntax.
///
/// Provides `convert_to::<T>()` method for cases where type inference
/// doesn't work well.
pub trait ConvertTo: Sized {
    /// Convert this value to the specified type.
    fn convert_to<To>(self) -> To
    where
        Self: Convert<To>;
}

impl<T> ConvertTo for T {
    #[inline]
    fn convert_to<To>(self) -> To
    where
        Self: Convert<To>,
    {
        self.convert()
    }
}

// Implement Convert for all types that implement ChannelConvert
impl<From, To> Convert<To> for From
where
    From: ChannelConvert<To>,
{
    #[inline]
    fn convert(self) -> To {
        ChannelConvert::convert(self)
    }
}

// =============================================================================
// Public conversion function - dispatches to SIMD or scalar
// =============================================================================

/// Convert image pixel data from one format to another.
/// Automatically uses SIMD-optimized paths when available.
pub(crate) fn convert_image(from: &Image, to: &mut Image) -> Result<()> {
    // Try SIMD path first
    if conversion_simd::try_convert_simd(from, to)? {
        return Ok(());
    }

    // Fall back to scalar implementation
    conversion_scalar::convert_image(from, to)
}
