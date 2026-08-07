//! Linux. See [`crate::platform`] for the surface every OS module implements.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use crate::gui::HostHandle;

/// Nothing to arrange: `assets/linux/com.cssodessa.darkroom.desktop` passes
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
    resolve_config_dir(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
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
mod tests {
    use super::*;

    #[test]
    fn config_dir_prefers_an_absolute_xdg_config_home() {
        assert_eq!(
            resolve_config_dir(Some("/xdg".into()), Some("/home/u".into())),
            Some(PathBuf::from("/xdg/darkroom")),
            "an absolute XDG_CONFIG_HOME wins over HOME"
        );
    }

    #[test]
    fn config_dir_falls_back_to_home_config() {
        // Unset, and — separately — set but relative, which the spec says to
        // ignore rather than resolve against the working directory.
        for xdg in [
            None,
            Some(OsString::from("relative/xdg")),
            Some(OsString::new()),
        ] {
            assert_eq!(
                resolve_config_dir(xdg.clone(), Some("/home/u".into())),
                Some(PathBuf::from("/home/u/.config/darkroom")),
                "{xdg:?} is not a usable XDG_CONFIG_HOME"
            );
        }
    }

    #[test]
    fn config_dir_is_none_without_a_usable_home() {
        for home in [
            None,
            Some(OsString::from("relative")),
            Some(OsString::new()),
        ] {
            assert_eq!(
                resolve_config_dir(None, home.clone()),
                None,
                "{home:?} is not a usable HOME"
            );
        }
    }
}
