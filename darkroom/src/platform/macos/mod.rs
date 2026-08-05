//! macOS. See [`crate::platform`] for the surface every OS module implements.

use std::process::Command;

use crate::gui::HostHandle;
use crate::gui::app::App;

pub(crate) mod open_files;

/// The delegate patch has to land after `EventLoop::new` built the delegate
/// and before `run` starts driving the loop, which is exactly where
/// [`crate::platform::route_opened_documents`] is called from.
///
/// `run_on_main` is what spans the remaining gap: a document double-clicked on
/// a *cold* app arrives before `App` exists, and the host buffers the task
/// until it does.
pub(super) fn route_opened_documents(handle: HostHandle) {
    open_files::install(move |path| {
        let _ = handle.run_on_main(move |app: &mut App| {
            app.open_document_at(path);
            true
        });
    });
}

pub(super) fn url_opener() -> Command {
    Command::new("open")
}
