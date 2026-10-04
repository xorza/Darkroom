//! FFI bindings to LibRaw's C API, linked statically from the `LibRaw` submodule. `build.rs`
//! generates the bindings for the target being built.

#![no_std]
#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    reason = "the names are LibRaw's, from its C headers"
)]
#![allow(clippy::all, reason = "the crate is bindgen output and nothing else")]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

unsafe extern "C" {
    /// Whether LibRaw reads a raw value of zero as a dead photosite rather than as data: its
    /// `internal_output_params.zero_is_bad`, set for Panasonic and some cameras its size table
    /// identifies. Valid after `libraw_open_*`. Defined in `shim/internal.cpp`.
    pub fn libraw_lumos_zero_is_bad(lr: *mut libraw_data_t) -> core::ffi::c_uint;

    /// A Fuji SuperCCD's rotated width — its `internal_output_params.fuji_width` — or 0 for any
    /// other sensor, whose photosites lie on the rows. Valid after `libraw_open_*`. Defined in
    /// `shim/internal.cpp`.
    pub fn libraw_lumos_fuji_width(lr: *mut libraw_data_t) -> core::ffi::c_uint;
}
