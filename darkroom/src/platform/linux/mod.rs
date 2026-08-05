//! Linux. See [`crate::platform`] for the surface every OS module implements.

use std::process::Command;

use crate::gui::HostHandle;

/// Nothing to arrange: `assets/linux/com.cssodessa.darkroom.desktop` passes
/// the path as `%f`, so a document opened from a file manager arrives in argv
/// and the CLI already reads it.
pub(super) fn route_opened_documents(_handle: HostHandle) {}

pub(super) fn url_opener() -> Command {
    Command::new("xdg-open")
}
