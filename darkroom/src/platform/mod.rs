//! What the editor has to do differently per operating system.
//!
//! Each supported OS gets a submodule implementing the same private surface,
//! bound to `sys` below; the functions here are the only way in, so callers
//! never carry a `#[cfg]` of their own. What a platform has nothing to do for
//! it implements empty, which is the difference between "this OS needs
//! nothing" and "nobody wrote it yet" — the first is written down, the second
//! fails to compile.

use std::path::PathBuf;

use crate::gui::HostHandle;

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
/// Only macOS has anything to do. A Linux desktop entry's `%f` and a Windows
/// shell association both put the path in argv, where the CLI already reads
/// it; Launch Services sends an Apple Event instead and never fills argv, so
/// without this the association in `assets/macos/Info.plist` would launch the
/// editor empty.
///
/// Call once, after the host is built and before it runs — that window is the
/// only one that works, for the reasons in `macos::open_files`.
pub(crate) fn route_opened_documents(handle: HostHandle) {
    sys::route_opened_documents(handle);
}

/// Where this OS keeps the editor's configuration, darkroom's own component
/// included — callers join a file name and nothing else. Naming that component
/// is per-OS: lowercase under XDG, capitalised on macOS and Windows.
///
/// Says where the directory belongs; creating it is the caller's business.
/// `None` when the environment names no home.
pub(crate) fn config_dir() -> Option<PathBuf> {
    sys::config_dir()
}

/// Open `url` in the user's default browser.
///
/// Non-blocking — the spawn returns immediately, so this is safe to call
/// mid-record, unlike the modal file dialogs. A missing opener logs and
/// degrades; there is no user-facing error surface yet.
pub(crate) fn open_url(url: &str) {
    if let Err(e) = sys::url_opener().arg(url).spawn() {
        tracing::warn!("failed to open url {url}: {e}");
    }
}
