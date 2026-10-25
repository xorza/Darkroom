//! [`ProcessedImage`]: an image LibRaw's own processing made.

use std::marker::PhantomData;
use std::ptr::NonNull;
use std::{fmt, slice};

use common::Size2us;
use libraw_sys as sys;

use crate::error::LibrawError;
use crate::libraw::Libraw;

/// An image LibRaw's own processing made, freed when dropped. It borrows the [`Libraw`] that made
/// it, which LibRaw requires to outlive it.
pub struct ProcessedImage<'a> {
    pub(crate) image: NonNull<sys::libraw_processed_image_t>,
    pub(crate) _libraw: PhantomData<&'a mut Libraw>,
}

/// A [`ProcessedImage`]'s samples, interleaved by color, and the shape they fill.
#[derive(Debug)]
pub struct ProcessedSamples<'a> {
    pub samples: &'a [u16],
    pub size: Size2us,
    /// The colors each pixel holds, interleaved.
    pub colors: usize,
}

impl ProcessedImage<'_> {
    /// The samples, checked against the shape LibRaw states and the bytes it allocated: pixels of
    /// 16-bit samples, as the processing was asked for.
    ///
    /// # Errors
    ///
    /// [`LibrawError::ProcessedShape`] for no pixels or samples of other than 16 bits,
    /// [`LibrawError::ProcessedSize`] when the image holds fewer bytes than its shape needs.
    pub fn samples(&self) -> Result<ProcessedSamples<'_>, LibrawError> {
        let header = unsafe { self.image.as_ref() };
        let (width, height) = (usize::from(header.width), usize::from(header.height));
        let (colors, bits) = (usize::from(header.colors), usize::from(header.bits));
        let shape = LibrawError::ProcessedShape {
            width,
            height,
            colors,
            bits,
        };
        if width == 0 || height == 0 || colors == 0 || bits != 16 {
            return Err(shape);
        }
        let len = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(colors))
            .ok_or(shape)?;
        let expected = len * size_of::<u16>();
        let actual = header.data_size as usize;
        if actual < expected {
            return Err(LibrawError::ProcessedSize { actual, expected });
        }
        // The samples run past the one-byte `data` field the header declares, so they are reached
        // through a pointer to the allocation rather than a reference to the field.
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "LibRaw aligns its 16-bit samples, as the assert below checks"
        )]
        let data = unsafe { (&raw const (*self.image.as_ptr()).data).cast::<u16>() };
        debug_assert!(data.is_aligned(), "LibRaw aligns its 16-bit samples");
        let samples = unsafe { slice::from_raw_parts(data, len) };
        Ok(ProcessedSamples {
            samples,
            size: Size2us::new(width, height),
            colors,
        })
    }
}

impl Drop for ProcessedImage<'_> {
    fn drop(&mut self) {
        unsafe { sys::libraw_dcraw_clear_mem(self.image.as_ptr()) };
    }
}

impl fmt::Debug for ProcessedImage<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessedImage").finish_non_exhaustive()
    }
}
