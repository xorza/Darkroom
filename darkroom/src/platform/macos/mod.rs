//! macOS. See [`crate::platform`] for the surface every OS module implements.

use std::ffi::OsString;
use std::path::PathBuf;
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

/// `~/Library/Application Support/Darkroom`.
pub(super) fn config_dir() -> Option<PathBuf> {
    // Not ~/Library/Preferences: NSUserDefaults owns those plists and rewrites
    // them behind a caching daemon, so a hand-edited file there won't survive.
    resolve_config_dir(std::env::var_os("HOME"))
}

/// Environment passed in, so resolution is testable without `set_var`.
fn resolve_config_dir(home: Option<OsString>) -> Option<PathBuf> {
    home.map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .map(|home| home.join("Library/Application Support/Darkroom"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_sits_under_application_support() {
        assert_eq!(
            resolve_config_dir(Some("/Users/u".into())),
            Some(PathBuf::from(
                "/Users/u/Library/Application Support/Darkroom"
            ))
        );
    }

    #[test]
    fn config_dir_is_none_without_a_usable_home() {
        for home in [
            None,
            Some(OsString::from("relative")),
            Some(OsString::new()),
        ] {
            assert_eq!(
                resolve_config_dir(home.clone()),
                None,
                "{home:?} is not a usable HOME"
            );
        }
    }
}
