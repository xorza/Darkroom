//! Windows. See [`crate::platform`] for the surface every OS module
//! implements.

use std::ffi::OsString;
use std::path::PathBuf;

use common::env;

use palantir::HostHandle;

/// Nothing to arrange: a shell file association passes the path in argv,
/// where the CLI already reads it.
pub(super) fn route_opened_documents<A>(_handle: HostHandle<A>, _open: fn(&mut A, PathBuf)) {}

/// `%APPDATA%\Darkroom` — roaming, so settings follow a domain user.
pub(super) fn config_dir() -> Option<PathBuf> {
    resolve_config_dir(env::var_os("APPDATA"))
}

/// Environment passed in, so resolution is testable without `set_var`.
fn resolve_config_dir(appdata: Option<OsString>) -> Option<PathBuf> {
    appdata
        .map(PathBuf::from)
        .filter(|root| root.is_absolute())
        .map(|root| root.join("Darkroom"))
}

#[cfg(test)]
mod tests;
