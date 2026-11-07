//! [`PathForm`]: what the text of a path input says, with no file system.

use std::path::{Component, Path};

/// What a path input's text is on this system. The other system's absolute forms are read as text,
/// as Python's `PureWindowsPath` and `PurePosixPath` read them
/// (<https://docs.python.org/3/library/pathlib.html>), since `Path` knows only this system's: a
/// document saved on one system can name a path outside its folder that way. Cost: a file a Unix
/// system names `C:\x` reads as another system's path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathForm {
    /// Empty, as an unset input holds.
    Unset,
    /// Absolute on this system.
    Absolute,
    /// Absolute on another system, or tied to a root or a drive with no folder to read it against:
    /// no folder here can complete it, so it stays as it is.
    Foreign,
    /// Relative to a folder.
    Relative,
}

impl PathForm {
    pub(crate) fn of(value: &str) -> Self {
        if value.is_empty() {
            return Self::Unset;
        }
        let path = Path::new(value);
        if path.is_absolute() {
            return Self::Absolute;
        }
        let rooted = matches!(
            path.components().next(),
            Some(Component::Prefix(_) | Component::RootDir)
        );
        if rooted || value.starts_with(r"\\") || has_drive_root(value) {
            Self::Foreign
        } else {
            Self::Relative
        }
    }
}

/// Whether `value` starts as Windows' `C:\` or `C:/` does.
const fn has_drive_root(value: &str) -> bool {
    matches!(
        value.as_bytes(),
        [drive, b':', b'\\' | b'/', ..] if drive.is_ascii_alphabetic()
    )
}

#[cfg(test)]
mod tests {
    use crate::core::document::path_form::PathForm;
    use std::path::Path;

    /// Each string is read on every OS: a form that this system calls absolute is `Absolute`, and
    /// otherwise `Foreign`, never `Relative`.
    #[test]
    fn each_systems_absolute_forms_are_never_relative() {
        for value in [
            "/x",
            "/data/a.fit",
            r"C:\x",
            "C:/x",
            "z:/x",
            r"\\server\share\x",
            r"\\?\C:\x",
        ] {
            let want = if Path::new(value).is_absolute() {
                PathForm::Absolute
            } else {
                PathForm::Foreign
            };
            assert_eq!(PathForm::of(value), want, "{value}");
        }
    }

    /// Text that only looks near a drive stays relative: a name of two letters, and a digit.
    #[test]
    fn a_relative_path_and_an_unset_one_are_told_apart() {
        assert_eq!(PathForm::of(""), PathForm::Unset);
        for value in ["a.tiff", "inputs/a.tiff", ".", "../x.tiff", "CC:/x", "1:/x"] {
            assert_eq!(PathForm::of(value), PathForm::Relative, "{value}");
        }
    }
}
