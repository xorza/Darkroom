//! [`Libraw`]: one open LibRaw instance, the file it parses in place, and what its callbacks
//! report.

use std::cell::Cell;
use std::ffi::{c_char, c_int, c_void};
use std::fmt;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::slice;

use common::CancelToken;
use libraw_sys as sys;

use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::raw::error::{LibrawCode, RawError};
use crate::math::size2us::Size2us;

/// One open LibRaw instance, the file bytes it parses in place, and the state its callbacks
/// write.
///
/// The one owner of the handle: every call that hands LibRaw the handle, and the one read of its
/// data, sit here, so the rest of the decoder reads LibRaw's state as plain Rust values. The file
/// is read whole and opened from memory on every system, so one datastream serves every file.
pub(super) struct Libraw {
    handle: NonNull<sys::libraw_data_t>,
    /// The file LibRaw parses in place, read until the handle is closed.
    file: Vec<u8>,
    /// What LibRaw's callbacks read and write, on the heap: LibRaw holds its address.
    hooks: Box<Hooks>,
}

/// The state LibRaw's callbacks reach through their user data.
#[derive(Debug)]
struct Hooks {
    /// Polled at each stage LibRaw reports, which stops the decode with
    /// [`LibrawCode::Cancelled`].
    cancel: CancelToken,
    /// Whether LibRaw met a value its format cannot hold and decoded on past it. LibRaw reports the
    /// first such value of a file alone.
    corrupt: Cell<bool>,
}

/// An image LibRaw's own processing made, freed when dropped. It borrows the [`Libraw`] that made
/// it, which LibRaw requires to outlive it.
pub(super) struct ProcessedImage<'a> {
    image: NonNull<sys::libraw_processed_image_t>,
    _libraw: PhantomData<&'a mut Libraw>,
}

/// A [`ProcessedImage`]'s samples, interleaved by colour, and the shape they fill.
#[derive(Debug)]
pub(super) struct ProcessedSamples<'a> {
    pub(super) samples: &'a [u16],
    pub(super) dimensions: ImageDimensions,
}

impl Libraw {
    /// Open the camera RAW file `file`, its decode stopped at the next stage LibRaw reports once
    /// `cancel` is set.
    ///
    /// # Errors
    ///
    /// [`RawError::Init`] when LibRaw cannot allocate its state, [`RawError::Open`] when it refuses
    /// the file.
    pub(super) fn open(file: Vec<u8>, cancel: &CancelToken) -> Result<Self, RawError> {
        let libraw = Self::init(file, cancel)?;
        // SAFETY: the handle is valid, and LibRaw reads the bytes in place until it is closed,
        // which `Drop` does before `file` is freed.
        let code = unsafe {
            sys::libraw_open_buffer(
                libraw.handle.as_ptr(),
                libraw.file.as_ptr().cast(),
                libraw.file.len(),
            )
        };
        LibrawCode::of(code).map_or(Ok(libraw), |code| Err(RawError::Open(code)))
    }

    /// A handle with both callbacks installed, holding `file`, nothing opened yet.
    fn init(file: Vec<u8>, cancel: &CancelToken) -> Result<Self, RawError> {
        // SAFETY: `libraw_init` returns a valid handle or null.
        let handle = NonNull::new(unsafe { sys::libraw_init(0) }).ok_or(RawError::Init)?;
        let hooks = Box::new(Hooks {
            cancel: cancel.clone(),
            corrupt: Cell::new(false),
        });
        let user_data = (&raw const *hooks).cast_mut().cast::<c_void>();
        // SAFETY: the handle is valid, and `hooks` stays at its heap address until `Drop` closes
        // the handle; LibRaw calls back only from inside the calls this type makes.
        unsafe {
            sys::libraw_set_dataerror_handler(handle.as_ptr(), Some(record_data_error), user_data);
            sys::libraw_set_progress_handler(handle.as_ptr(), Some(poll_cancel), user_data);
        }
        Ok(Self {
            handle,
            file,
            hooks,
        })
    }

    /// The size of the file LibRaw holds in memory.
    pub(super) const fn file_len(&self) -> usize {
        self.file.len()
    }

    /// LibRaw's state.
    pub(super) const fn data(&self) -> &sys::libraw_data_t {
        // SAFETY: the handle is valid while `self` lives, and LibRaw writes its state only inside
        // the calls this type makes, which take `&mut self`.
        unsafe { self.handle.as_ref() }
    }

    /// The parameters LibRaw's own processing runs with.
    pub(super) const fn params_mut(&mut self) -> &mut sys::libraw_output_params_t {
        // SAFETY: as `data`, and `&mut self` makes this the only reference.
        unsafe { &mut self.handle.as_mut().params }
    }

    /// Decode the sensor data.
    ///
    /// # Errors
    ///
    /// [`RawError::Unpack`] when LibRaw fails, and [`RawError::CorruptData`] when it decoded past
    /// values the format cannot hold.
    pub(super) fn unpack(&mut self) -> Result<(), RawError> {
        // SAFETY: the handle is valid and open.
        let code = unsafe { sys::libraw_unpack(self.handle.as_ptr()) };
        if let Some(code) = LibrawCode::of(code) {
            return Err(RawError::Unpack(code));
        }
        if self.hooks.corrupt.get() {
            return Err(RawError::CorruptData);
        }
        Ok(())
    }

    /// The unpacked raw buffer, `raw_height` rows of `raw_pitch` bytes, or `None` when LibRaw
    /// unpacked none — a sensor it delivers already processed.
    pub(super) fn raw_image(&self) -> Option<&[u16]> {
        let data = self.data();
        let image = NonNull::new(data.rawdata.raw_image)?;
        let len = usize::from(data.sizes.raw_height) * data.sizes.raw_pitch as usize / 2;
        // SAFETY: LibRaw allocated the buffer for `raw_height` rows of `raw_pitch` bytes, and frees
        // it no sooner than the handle, which `self` borrows.
        Some(unsafe { slice::from_raw_parts(image.as_ptr(), len) })
    }

    /// Whether LibRaw reads a raw zero as a dead photosite: its `zero_is_bad`, set for Panasonic
    /// and some cameras its size table identifies. Settled by the open, before `unpack`.
    pub(super) fn zero_is_bad(&self) -> bool {
        // SAFETY: the handle is valid and open; the shim only reads.
        unsafe { sys::libraw_lumos_zero_is_bad(self.handle.as_ptr()) != 0 }
    }

    /// Run LibRaw's own processing under [`Self::params_mut`] and take the image it makes.
    ///
    /// # Errors
    ///
    /// [`RawError::Process`] when either step fails.
    pub(super) fn process(&mut self) -> Result<ProcessedImage<'_>, RawError> {
        // SAFETY: the handle is valid and unpacked.
        let code = unsafe { sys::libraw_dcraw_process(self.handle.as_ptr()) };
        if let Some(code) = LibrawCode::of(code) {
            return Err(RawError::Process(code));
        }
        let mut code = 0;
        // SAFETY: the handle is valid and processed.
        let image =
            unsafe { sys::libraw_dcraw_make_mem_image(self.handle.as_ptr(), &raw mut code) };
        let image = NonNull::new(image).map(|image| ProcessedImage {
            image,
            _libraw: PhantomData,
        });
        match (image, LibrawCode::of(code)) {
            (Some(image), None) => Ok(image),
            (_, code) => Err(RawError::Process(code.unwrap_or(LibrawCode::Unspecified))),
        }
    }
}

impl Drop for Libraw {
    fn drop(&mut self) {
        // Runs ahead of the fields, as LibRaw needs: it may still point into `file` and `hooks`.
        // SAFETY: the handle came from `libraw_init` and is closed once.
        unsafe { sys::libraw_close(self.handle.as_ptr()) };
    }
}

impl fmt::Debug for Libraw {
    /// The file's size, not its bytes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Libraw")
            .field("file_bytes", &self.file.len())
            .field("hooks", &self.hooks)
            .finish_non_exhaustive()
    }
}

impl ProcessedImage<'_> {
    /// The samples, checked against the shape LibRaw states and the bytes it allocated: one or
    /// three colours of 16 bits, as the processing was asked for.
    ///
    /// # Errors
    ///
    /// [`RawError::ProcessedShape`] for any other shape, [`RawError::ProcessedSize`] when the
    /// image holds fewer bytes than its shape needs.
    pub(super) fn samples(&self) -> Result<ProcessedSamples<'_>, RawError> {
        // SAFETY: LibRaw made the image and frees it only in `Drop`.
        let header = unsafe { self.image.as_ref() };
        let (width, height) = (usize::from(header.width), usize::from(header.height));
        let (colors, bits) = (usize::from(header.colors), usize::from(header.bits));
        let shape = RawError::ProcessedShape {
            width,
            height,
            colors,
            bits,
        };
        if width == 0 || height == 0 || !matches!(colors, 1 | 3) || bits != 16 {
            return Err(shape);
        }
        let len = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(colors))
            .ok_or(shape)?;
        let expected = len * size_of::<u16>();
        let actual = header.data_size as usize;
        if actual < expected {
            return Err(RawError::ProcessedSize { actual, expected });
        }
        // The samples run past the one-byte `data` field the header declares, so they are reached
        // through a pointer to the allocation rather than a reference to the field.
        // SAFETY: LibRaw allocated `data_size` bytes from `data` on, 16-bit aligned.
        let data = unsafe { (&raw const (*self.image.as_ptr()).data).cast::<u16>() };
        debug_assert!(data.is_aligned(), "LibRaw aligns its 16-bit samples");
        // SAFETY: `len` samples fit the `data_size` bytes checked above, alive as long as `self`.
        let samples = unsafe { slice::from_raw_parts(data, len) };
        Ok(ProcessedSamples {
            samples,
            dimensions: ImageDimensions::new(Size2us::new(width, height), colors),
        })
    }
}

impl Drop for ProcessedImage<'_> {
    fn drop(&mut self) {
        // SAFETY: LibRaw made the image with `libraw_dcraw_make_mem_image`, and it is freed once.
        unsafe { sys::libraw_dcraw_clear_mem(self.image.as_ptr()) };
    }
}

impl fmt::Debug for ProcessedImage<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessedImage").finish_non_exhaustive()
    }
}

/// LibRaw's data-error callback: records the error in the [`Hooks`] `user_data` points at, and
/// writes nothing to stderr, as LibRaw's default callback does.
unsafe extern "C" fn record_data_error(user_data: *mut c_void, _file: *const c_char, _offset: i64) {
    // SAFETY: `user_data` is the `Hooks` `Libraw::init` registered, alive while the handle is.
    let hooks = unsafe { &*user_data.cast::<Hooks>() };
    hooks.corrupt.set(true);
}

/// LibRaw's progress callback: a nonzero answer stops the decode with `LIBRAW_CANCELLED_BY_CALLBACK`.
unsafe extern "C" fn poll_cancel(
    user_data: *mut c_void,
    _stage: sys::LibRaw_progress,
    _iteration: c_int,
    _expected: c_int,
) -> c_int {
    // SAFETY: as `record_data_error`.
    let hooks = unsafe { &*user_data.cast::<Hooks>() };
    c_int::from(hooks.cancel.is_cancelled())
}

#[cfg(test)]
pub(crate) mod internals {
    use common::CancelToken;
    use libraw_sys as sys;

    use crate::io::raw::error::{LibrawCode, RawError};
    use crate::io::raw::libraw::Libraw;

    /// What LibRaw's `open_bayer` reads a synthetic sensor dump as, beside its samples.
    #[derive(Debug, Clone, Copy, Default)]
    pub(crate) struct BayerDump {
        /// `procflags`: 2 marks zeros dead.
        pub(crate) procflags: u8,
        pub(crate) black: u32,
        /// `otherflags`: its high nibble narrows the 16 bits a sample is read as.
        pub(crate) otherflags: u8,
        /// The raw area `[top, left, bottom, right)` LibRaw measures the black on, as a camera's
        /// table names its masked pixels.
        pub(crate) mask: Option<[i32; 4]>,
    }

    impl Libraw {
        /// Set the orientation LibRaw saved at unpack, which its processing turns by.
        pub(crate) const fn set_saved_flip(&mut self, flip: i32) {
            // SAFETY: as `params_mut`.
            unsafe { self.handle.as_mut().rawdata.sizes.flip = flip };
        }

        /// `samples` as LibRaw's `open_bayer` reads a sensor dump: 16-bit little-endian, `side`
        /// square under one-pixel margins, RGGB, maximum 65535, polling `cancel`.
        pub(crate) fn open_bayer(
            samples: &[u16],
            side: u16,
            dump: BayerDump,
            cancel: &CancelToken,
        ) -> Result<Self, RawError> {
            let file = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect();
            let mut libraw = Self::init(file, cancel)?;
            let len = libraw.file.len() as u32;
            // SAFETY: the handle is valid, and LibRaw reads the bytes in place until it is closed.
            let code = unsafe {
                sys::libraw_open_bayer(
                    libraw.handle.as_ptr(),
                    libraw.file.as_mut_ptr(),
                    len,
                    side,
                    side,
                    1,
                    1,
                    1,
                    1,
                    dump.procflags,
                    0x94, // RGGB in LibRaw's filter byte
                    0,
                    u32::from(dump.otherflags),
                    dump.black,
                )
            };
            if let Some(code) = LibrawCode::of(code) {
                return Err(RawError::Open(code));
            }
            if let Some(mask) = dump.mask {
                // SAFETY: as `params_mut`; `unpack` reads the mask.
                unsafe { libraw.handle.as_mut().sizes.mask[0] = mask };
            }
            Ok(libraw)
        }
    }
}
