//! Linux. See [`crate::platform`] for the surface every OS module implements.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use crate::gui::HostHandle;

/// Nothing to arrange: `packaging/linux/com.cssodessa.darkroom.desktop` passes
/// the path as `%f`, so a document opened from a file manager arrives in argv
/// and the CLI already reads it.
pub(super) fn route_opened_documents(_handle: HostHandle) {}

pub(super) fn url_opener() -> Command {
    Command::new("xdg-open")
}

/// `$XDG_CONFIG_HOME/darkroom`, or `~/.config/darkroom` when unset.
pub(super) fn config_dir() -> Option<PathBuf> {
    // Flatpak points XDG_CONFIG_HOME at its per-app config directory; a
    // hardcoded ~/.config would land outside what the sandbox grants.
    resolve_config_dir(env::var_os("XDG_CONFIG_HOME"), env::var_os("HOME"))
}

/// Environment passed in, so resolution is testable without `set_var`.
fn resolve_config_dir(
    xdg_config_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    // is_absolute, not is_some: an empty var is set, and the spec says to
    // ignore a relative XDG_CONFIG_HOME rather than resolve it against cwd.
    xdg_config_home
        .map(PathBuf::from)
        .filter(|root| root.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|home| home.is_absolute())
                .map(|home| home.join(".config"))
        })
        .map(|root| root.join("darkroom"))
}

#[cfg(test)]
mod tests;
