//! FFI bindings to LibRaw's C API, linked statically from the `LibRaw` submodule. `build.rs`
//! generates them for the target being built.
//!
//! A private module, not a `*-sys` crate: the Cargo book's `*-sys` convention lets several
//! wrappers share the one package that `links` a native library, and this crate is LibRaw's only
//! user, so a module keeps every call into LibRaw inside it by visibility.

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

// Linked for LibRaw's deflate DNG decoder, which `build.rs` compiles against its headers; no Rust
// here calls it.
use libz_sys as _;
