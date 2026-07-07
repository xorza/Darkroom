//! Embed the Windows application icon into the executable.
//!
//! On Windows, compiles `assets/icons/darkroom.ico` into the `.exe` as a
//! Win32 resource so Explorer, the taskbar, and Alt-Tab show the app icon.
//! No-op on every other platform (the `winresource` build-dependency is
//! `cfg(windows)`-gated in Cargo.toml, so it isn't even compiled elsewhere).
//! The in-window title-bar icon is set separately at runtime — see
//! `load_icon` in `src/main.rs`.

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icons/darkroom.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icons/darkroom.ico");
        res.compile()
            .expect("embed darkroom.ico into the Windows executable");
    }
}
