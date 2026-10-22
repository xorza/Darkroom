//! [`Libraw`]: one open LibRaw instance, the file it parses in place, and what its callbacks
//! report.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{fmt, mem, slice};

use common::CancelToken;
use libraw_sys as sys;

use crate::image::channel_layout::ChannelLayout;
use crate::image::image_dimensions::ImageDimensions;
use crate::io::raw::error::{LibrawCode, RawError};
use crate::io::raw::raw_decoder::{CodecFacts, CrxCoding, RawDecoder};
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

/// The state LibRaw's callbacks reach through their user data. LibRaw calls them from its decoder's
/// OpenMP threads as well as from the calling one, so the state is shared across threads.
#[derive(Debug)]
struct Hooks {
    /// Polled at each stage LibRaw reports, which stops the decode with
    /// [`LibrawCode::Cancelled`].
    cancel: CancelToken,
    /// Whether LibRaw met a value its format cannot hold and decoded on past it. LibRaw reports the
    /// first such value of a file alone.
    corrupt: AtomicBool,
}

/// An image LibRaw's own processing made, freed when dropped. It borrows the [`Libraw`] that made
/// it, which LibRaw requires to outlive it.
pub(super) struct ProcessedImage<'a> {
    image: NonNull<sys::libraw_processed_image_t>,
    _libraw: PhantomData<&'a mut Libraw>,
}

/// A [`ProcessedImage`]'s samples, interleaved by color, and the shape they fill.
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
        let handle = NonNull::new(unsafe { sys::libraw_init(0) }).ok_or(RawError::Init)?;
        let hooks = Box::new(Hooks {
            cancel: cancel.clone(),
            corrupt: AtomicBool::new(false),
        });
        let user_data = (&raw const *hooks).cast_mut().cast::<c_void>();
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

    /// The file's bytes, the handle closed: a header read hands them to the load, which opens
    /// them again rather than read the file again.
    pub(super) fn into_file(mut self) -> Vec<u8> {
        mem::take(&mut self.file)
    }

    /// The size of the file LibRaw holds in memory.
    pub(super) const fn file_len(&self) -> usize {
        self.file.len()
    }

    /// Free the file once `unpack` read the sensor data out of it: LibRaw drops its stream over
    /// the bytes, and what a mosaic is read from after — its state and the raw buffer — lives in
    /// LibRaw's own memory. Its own processing reads the file again, so it refuses a handle
    /// whose file is gone.
    pub(super) fn release_file(&mut self) {
        unsafe { sys::libraw_recycle_datastream(self.handle.as_ptr()) };
        self.file = Vec::new();
    }

    /// LibRaw's state.
    pub(super) const fn data(&self) -> &sys::libraw_data_t {
        unsafe { self.handle.as_ref() }
    }

    /// The decoder LibRaw chose for the open file, with its codec's facts — see [`RawDecoder`].
    pub(super) fn decoder(&self) -> RawDecoder {
        let handle = self.handle.as_ptr();
        let mut info = sys::libraw_decoder_info_t::default();
        let (mut enc_type, mut image_levels) = (0, 0);
        let (name, floating_point, fuji_lossless, crx) = unsafe {
            sys::libraw_get_decoder_info(handle, &raw mut info);
            let name = if info.decoder_name.is_null() {
                &[][..]
            } else {
                CStr::from_ptr(info.decoder_name).to_bytes()
            };
            (
                name,
                sys::libraw_lumos_is_floating_point(handle) != 0,
                sys::libraw_lumos_fuji_lossless(handle) != 0,
                sys::libraw_lumos_crx_coding(handle, &raw mut enc_type, &raw mut image_levels) != 0,
            )
        };
        RawDecoder::classify(CodecFacts {
            name,
            floating_point,
            fuji_lossless,
            crx: crx.then_some(CrxCoding {
                enc_type,
                image_levels,
            }),
        })
    }

    /// The camera's clock at capture as ISO 8601 text with no zone, or `None` when the file states
    /// no time. LibRaw reads it as a local time of this machine; read back in the same zone, it is
    /// the camera's own clock, in whatever zone that was set to.
    pub(super) fn camera_clock(&self) -> Option<String> {
        let mut text: [c_char; 20] = [0; 20];
        let written = unsafe {
            sys::libraw_lumos_camera_clock(self.handle.as_ptr(), text.as_mut_ptr(), text.len())
        };
        (written != 0).then(|| {
            unsafe { CStr::from_ptr(text.as_ptr()) }
                .to_string_lossy()
                .into_owned()
        })
    }

    /// Whether the sensor is a Fuji SuperCCD, its photosites laid out 45° to the rows.
    pub(super) fn super_ccd(&self) -> bool {
        unsafe { sys::libraw_lumos_fuji_width(self.handle.as_ptr()) != 0 }
    }

    /// The parameters LibRaw's own processing runs with.
    pub(super) const fn params_mut(&mut self) -> &mut sys::libraw_output_params_t {
        unsafe { &mut self.handle.as_mut().params }
    }

    /// Decode the sensor data, a decoder that parallelizes itself on `threads` threads.
    ///
    /// # Errors
    ///
    /// [`RawError::Unpack`] when LibRaw fails, and [`RawError::CorruptData`] when it decoded past
    /// values the format cannot hold.
    pub(super) fn unpack(&mut self, threads: usize) -> Result<(), RawError> {
        let threads = c_int::try_from(threads.max(1)).unwrap_or(c_int::MAX);
        let code = unsafe {
            sys::libraw_lumos_set_decode_threads(threads);
            sys::libraw_unpack(self.handle.as_ptr())
        };
        if let Some(code) = LibrawCode::of(code) {
            return Err(RawError::Unpack(code));
        }
        if self.hooks.corrupt.load(Ordering::Relaxed) {
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
        Some(unsafe { slice::from_raw_parts(image.as_ptr(), len) })
    }

    /// Run LibRaw's own processing under [`Self::params_mut`] and take the image it makes.
    ///
    /// # Errors
    ///
    /// [`RawError::Process`] when either step fails.
    pub(super) fn process(&mut self) -> Result<ProcessedImage<'_>, RawError> {
        assert!(
            !self.file.is_empty(),
            "LibRaw's processing reads the file, which was released"
        );
        let code = unsafe { sys::libraw_dcraw_process(self.handle.as_ptr()) };
        if let Some(code) = LibrawCode::of(code) {
            return Err(RawError::Process(code));
        }
        let mut code = 0;
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
    /// three colors of 16 bits, as the processing was asked for.
    ///
    /// # Errors
    ///
    /// [`RawError::ProcessedShape`] for any other shape, [`RawError::ProcessedSize`] when the
    /// image holds fewer bytes than its shape needs.
    pub(super) fn samples(&self) -> Result<ProcessedSamples<'_>, RawError> {
        let header = unsafe { self.image.as_ref() };
        let (width, height) = (usize::from(header.width), usize::from(header.height));
        let (colors, bits) = (usize::from(header.colors), usize::from(header.bits));
        let shape = RawError::ProcessedShape {
            width,
            height,
            colors,
            bits,
        };
        let Ok(layout) = ChannelLayout::try_from(colors) else {
            return Err(shape);
        };
        if width == 0 || height == 0 || bits != 16 {
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
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "LibRaw aligns its 16-bit samples, as the assert below checks"
        )]
        let data = unsafe { (&raw const (*self.image.as_ptr()).data).cast::<u16>() };
        debug_assert!(data.is_aligned(), "LibRaw aligns its 16-bit samples");
        let samples = unsafe { slice::from_raw_parts(data, len) };
        Ok(ProcessedSamples {
            samples,
            dimensions: ImageDimensions::new(Size2us::new(width, height), layout),
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

/// LibRaw's data-error callback: records the error in the [`Hooks`] `user_data` points at, and
/// writes nothing to stderr, as LibRaw's default callback does.
unsafe extern "C" fn record_data_error(user_data: *mut c_void, _file: *const c_char, _offset: i64) {
    let hooks = unsafe { &*user_data.cast::<Hooks>() };
    hooks.corrupt.store(true, Ordering::Relaxed);
}

/// LibRaw's progress callback: a nonzero answer stops the decode with `LIBRAW_CANCELLED_BY_CALLBACK`.
unsafe extern "C" fn poll_cancel(
    user_data: *mut c_void,
    _stage: sys::LibRaw_progress,
    _iteration: c_int,
    _expected: c_int,
) -> c_int {
    let hooks = unsafe { &*user_data.cast::<Hooks>() };
    c_int::from(hooks.cancel.is_cancelled())
}

#[cfg(test)]
pub(crate) mod internals {
    use common::CancelToken;
    use imaginarium::Buffer2;
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
        #[cfg(feature = "real-data")]
        pub(crate) const fn set_saved_flip(&mut self, flip: i32) {
            unsafe { self.handle.as_mut().rawdata.sizes.flip = flip };
        }

        /// `samples` as LibRaw's `open_bayer` reads a sensor dump: 16-bit little-endian, square
        /// under one-pixel margins, RGGB, maximum 65535, polling `cancel`.
        pub(crate) fn open_bayer(
            samples: &Buffer2<u16>,
            dump: BayerDump,
            cancel: &CancelToken,
        ) -> Result<Self, RawError> {
            assert_eq!(samples.width(), samples.height(), "a square dump");
            let side = u16::try_from(samples.width()).expect("a side LibRaw's call can state");
            let file = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect();
            let mut libraw = Self::init(file, cancel)?;
            let len = libraw.file.len() as u32;
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
                unsafe { libraw.handle.as_mut().sizes.mask[0] = mask };
            }
            Ok(libraw)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::ptr;
    use std::sync::atomic::{AtomicBool, Ordering};

    use common::{CancelToken, thread};

    use crate::io::raw::libraw::{Hooks, record_data_error};

    /// LibRaw's compressed-Fuji decoder reports a data error from each of its OpenMP threads, so
    /// the callback runs on several threads at once against one `Hooks`.
    #[test]
    fn data_errors_from_several_threads_are_recorded() {
        let hooks = Hooks {
            cancel: CancelToken::new(),
            corrupt: AtomicBool::new(false),
        };
        let shared = &hooks;
        thread::scope(|scope| {
            for offset in 0..4 {
                scope.spawn("lumos-test", move || {
                    let user_data = (&raw const *shared).cast_mut().cast::<c_void>();
                    unsafe { record_data_error(user_data, ptr::null(), offset) };
                });
            }
        });
        assert!(hooks.corrupt.load(Ordering::Relaxed));
    }
}
