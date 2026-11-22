//! Regenerates `libraw/src/sys/bindings.rs` from LibRaw's headers: run it when the LibRaw
//! submodule moves, with libclang installed (Xcode tools on macOS, `winget install LLVM.LLVM` on
//! Windows, `libclang-dev` on Linux). `cargo run --manifest-path libraw/bindgen/Cargo.toml`.
//!
//! The bindings are committed, not generated each build, as the bindgen book allows for a vendored
//! library pinned at a release tag (<https://rust-lang.github.io/rust-bindgen/>): no build needs
//! libclang. One file serves every target the workspace builds, all 64-bit: LibRaw's structs hold
//! fixed-width integers, `time_t`, pointers and `char`, and `time_t` and `FILE` are written here
//! in place of the platform typedefs clang would name. An enum of no negative value reads as
//! `c_uint` here where MSVC makes it `int`: the same 32 bits at every call. Each struct's size and
//! field offsets are asserted at compile time, so a target whose C layout differs fails to build
//! rather than misreading LibRaw, and a test holds the bindings' LibRaw version to the linked
//! library's, so a submodule moved without a regeneration fails it. A standalone package, outside the workspace, so bindgen builds only here.

#![expect(
    clippy::disallowed_methods,
    reason = "a developer tool that reads cargo's environment at run time"
)]

use std::env;
use std::path::PathBuf;

include!("../../defines.rs");

/// The C library's types LibRaw's API names, written once for every 64-bit target: `time_t` is
/// 64-bit on each (LP64 Unix, and MSVC on x64, which refuses `_USE_32BIT_TIME_T`), and `FILE` is
/// only ever behind a pointer.
const PORTABLE_TYPES: &str = "\
pub type time_t = i64;
/// The C `FILE`, opaque: LibRaw's API only hands it around behind a pointer.
#[repr(C)]
pub struct FILE {
    _opaque: [u8; 0],
}";

#[expect(
    clippy::print_stdout,
    reason = "a developer tool reports the file it wrote"
)]
fn main() {
    let crate_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("run through cargo"));
    let libraw = crate_dir
        .parent()
        .expect("the tool sits in libraw/")
        .to_path_buf();
    let header = libraw.join("LibRaw/libraw/libraw.h");
    let out = libraw.join("src/sys/bindings.rs");
    bindgen::Builder::default()
        .header(header.to_str().expect("UTF-8 path"))
        .clang_args(DEFINES.iter().map(|define| format!("-D{define}")))
        .use_core()
        .ctypes_prefix("core::ffi")
        .allowlist_function("libraw_.*")
        .allowlist_type("libraw_.*")
        .allowlist_type("LibRaw_errors")
        .allowlist_var("LIBRAW_.*")
        // The C library's own: the names C reserves for it in every scope (C11 7.1.3: two
        // underscores, or one and a capital), MSVC's `FILE` struct, and the typedefs `FILE`
        // reaches, so no libc's internals reach the file.
        .blocklist_type("__.*|_[A-Z].*|_iobuf|FILE|fpos_t|time_t")
        .raw_line(PORTABLE_TYPES)
        .derive_debug(true)
        .derive_default(true)
        .generate_comments(false)
        .formatter(bindgen::Formatter::Prettyplease)
        .generate()
        .expect("LibRaw's headers parse; bindgen needs libclang")
        .write_to_file(&out)
        .expect("libraw/src/sys/ is writable");
    println!("wrote {}", out.display());
}
