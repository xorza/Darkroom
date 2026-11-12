//! What the editor has to do differently per operating system.
//!
//! Each supported OS gets a submodule implementing the same private surface, bound to `sys`
//! below; the functions here are the only way in, so callers carry no `#[cfg]`. What a platform
//! has nothing to do for it implements empty: "this OS needs nothing" is written down, and
//! "nobody wrote it yet" fails to compile.

use std::path::PathBuf;

use palantir::HostHandle;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
use linux as sys;
#[cfg(target_os = "macos")]
use macos as sys;
#[cfg(target_os = "windows")]
use windows as sys;

/// Arrange for documents the desktop hands the app to reach the editor.
///
/// Only macOS has anything to do: a Linux desktop entry's `%f` and a Windows shell association
/// put the path in argv, where the CLI reads it, while Launch Services sends an Apple Event and
/// never fills argv.
///
/// Each document is handed to `open` on the main thread, once the app exists.
///
/// Call once, after the host is built and before it runs, the only window that works, for the
/// reasons in `macos::open_files`.
pub(crate) fn route_opened_documents<A>(handle: HostHandle<A>, open: fn(&mut A, PathBuf)) {
    sys::route_opened_documents(handle, open);
}

/// Where this OS keeps the editor's configuration, darkroom's own component included, so callers
/// join a file name and nothing else. The component is per-OS: lowercase under XDG, capitalised
/// on macOS and Windows.
///
/// Creating the directory is the caller's business. `None` when the environment names no home.
pub(crate) fn config_dir() -> Option<PathBuf> {
    sys::config_dir()
}
