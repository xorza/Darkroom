use crate::data::path_form::PathForm;
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
