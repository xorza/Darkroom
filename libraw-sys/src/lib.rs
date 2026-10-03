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
