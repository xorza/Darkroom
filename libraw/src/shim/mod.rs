//! The accessors `shim/internal.cpp` compiles beside LibRaw, for state its C API does not expose.

use crate::sys::libraw_data_t;

unsafe extern "C" {
    /// Set how many threads LibRaw's tiled decoders run the calling thread's next decodes on: its
    /// OpenMP thread count, which applies to the parallel regions the calling thread starts. A
    /// no-op when LibRaw was built without OpenMP.
    pub(crate) fn libraw_lumos_set_decode_threads(threads: core::ffi::c_int);

    /// Whether LibRaw was built with OpenMP, and so decodes its tiled formats in parallel.
    #[cfg(test)]
    pub(crate) fn libraw_lumos_openmp() -> core::ffi::c_uint;

    /// The width of a Fuji SuperCCD's 45° layout, 0 for any other sensor: its
    /// `internal_output_params.fuji_width`, set by `libraw_open_*`.
    pub(crate) fn libraw_lumos_fuji_width(lr: *mut libraw_data_t) -> core::ffi::c_uint;

    /// Whether the file's samples are floating-point — a float DNG, which `libraw_unpack`
    /// converts to integers by default. Valid after `libraw_open_*`.
    pub(crate) fn libraw_lumos_is_floating_point(lr: *mut libraw_data_t) -> core::ffi::c_uint;

    /// Whether a compressed Fuji RAF is lossless: its `unpacker_data.fuji_lossless`, which its
    /// header sets at `libraw_open_*`. Meaningless for any other file.
    pub(crate) fn libraw_lumos_fuji_lossless(lr: *mut libraw_data_t) -> core::ffi::c_int;

    /// The camera's clock at capture, `YYYY-MM-DDTHH:MM:SS`, written NUL-terminated into the
    /// `capacity` bytes at `text`: LibRaw's `other.timestamp` read back in the zone it was made
    /// in. Returns 0, writing nothing usable, when the file states no time or `capacity` is under
    /// 20.
    pub(crate) fn libraw_lumos_camera_clock(
        lr: *mut libraw_data_t,
        text: *mut core::ffi::c_char,
        capacity: usize,
    ) -> core::ffi::c_uint;

    /// The coding of a Canon CR3's selected track: its CMP1 header's `encType` and
    /// `imageLevels`, written through the pointers. Returns 0, writing nothing, when no track of
    /// those the file holds is selected — any file but a CR3. Valid after `libraw_open_*`.
    pub(crate) fn libraw_lumos_crx_coding(
        lr: *mut libraw_data_t,
        enc_type: *mut core::ffi::c_int,
        image_levels: *mut core::ffi::c_int,
    ) -> core::ffi::c_uint;
}

#[cfg(test)]
mod tests;
