//! Windows. See [`crate::platform`] for the surface every OS module
//! implements.

use std::ffi::OsString;
use std::path::PathBuf;
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

/// `%APPDATA%\Darkroom` — roaming, so settings follow a domain user.
pub(super) fn config_dir() -> Option<PathBuf> {
    resolve_config_dir(std::env::var_os("APPDATA"))
}

/// Environment passed in, so resolution is testable without `set_var`.
fn resolve_config_dir(appdata: Option<OsString>) -> Option<PathBuf> {
    appdata
        .map(PathBuf::from)
        .filter(|root| root.is_absolute())
        .map(|root| root.join("Darkroom"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_sits_under_appdata() {
        assert_eq!(
            resolve_config_dir(Some(r"C:\Users\u\AppData\Roaming".into())),
            Some(PathBuf::from(r"C:\Users\u\AppData\Roaming\Darkroom"))
        );
    }

    #[test]
    fn config_dir_is_none_without_a_usable_appdata() {
        for appdata in [
            None,
            Some(OsString::from("relative")),
            Some(OsString::new()),
        ] {
            assert_eq!(
                resolve_config_dir(appdata.clone()),
                None,
                "{appdata:?} is not a usable %APPDATA%"
            );
        }
    }
}
