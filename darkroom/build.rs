//! Embed the Windows application icon into the executable.
//!
//! For a Windows target, compiles `assets/icons/darkroom.ico` into the `.exe` as a Win32 resource
//! so Explorer, the taskbar, and Alt-Tab show the app icon. The target is read from
//! `CARGO_CFG_TARGET_OS`: a build script runs on the host, so its own `cfg(windows)` is the
//! host's, as winresource's README warns. A build from another host needs the resource compiler
//! winresource names: `llvm-rc` for MSVC, MinGW's `windres` for GNU. The in-window title-bar icon
//! is set separately at runtime — see `load_icon` in `src/main.rs`.

#![expect(
    clippy::disallowed_methods,
    reason = "a build script reads cargo's environment at build time"
)]

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=assets/icons/darkroom.ico");
    if env::var("CARGO_CFG_TARGET_OS").expect("cargo states the target OS") != "windows" {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icons/darkroom.ico");
    res.compile()
        .expect("embed darkroom.ico into the Windows executable");
}
