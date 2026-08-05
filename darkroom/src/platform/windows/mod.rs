//! Windows. See [`crate::platform`] for the surface every OS module
//! implements.

use std::process::Command;

use crate::gui::HostHandle;

/// Nothing to arrange: a shell file association passes the path in argv,
/// where the CLI already reads it.
pub(super) fn route_opened_documents(_handle: HostHandle) {}

pub(super) fn url_opener() -> Command {
    let mut command = Command::new("cmd");
    // `start` treats its first quoted argument as the window title, so an
    // empty title has to go in before the URL.
    command.args(["/C", "start", ""]);
    command
}
