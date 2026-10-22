//! Native file-picker dialogs (rfd). Project byte⇄type plumbing lives in
//! `crate::core::io::document`; this side hands paths off to that GUI-free
//! module. Failures degrade — a cancelled or failed pick returns `None` —
//! rather than crashing. Everything else the shell does for us is in
//! `crate::platform`.

use std::path::{Path, PathBuf};

use common::fs;

use crate::core::io::document;

fn file_dialog(start: Option<&Path>) -> rfd::FileDialog {
    let mut dialog = rfd::FileDialog::new();
    if let Some(parent) = start.and_then(Path::parent) {
        dialog = dialog.set_directory(parent);
    }
    dialog
}

pub(crate) fn pick_project_open_path(start: Option<&Path>) -> Option<PathBuf> {
    file_dialog(start)
        .add_filter("Darkroom project", &[document::EXTENSION])
        .pick_file()
}

/// The native dialog confirms overwriting the name the user typed, not the one
/// [`document::with_extension`] makes of it — so when that one already exists,
/// the dialog asks again with it filled in, and its own overwrite prompt
/// answers for the file that will be replaced. A second save dialog rather than
/// a message box, because rfd shows message boxes on Linux through zenity,
/// which a desktop may lack, and the save then fails with nothing on screen.
pub(crate) fn pick_project_save_path(start: Option<&Path>) -> Option<PathBuf> {
    let mut picked = project_save_dialog(start).save_file()?;
    while let Some(target) = unconfirmed_target(&picked) {
        let mut dialog = project_save_dialog(Some(&target));
        if let Some(name) = target.file_name().and_then(|name| name.to_str()) {
            dialog = dialog.set_file_name(name);
        }
        picked = dialog.save_file()?;
    }
    Some(document::with_extension(picked))
}

fn project_save_dialog(start: Option<&Path>) -> rfd::FileDialog {
    file_dialog(start).add_filter("Darkroom project", &[document::EXTENSION])
}

/// The file a save to `picked` would replace without the dialog having asked
/// about it: the extended name, when it differs from the picked one and exists.
fn unconfirmed_target(picked: &Path) -> Option<PathBuf> {
    let target = document::with_extension(picked.to_path_buf());
    (target != picked && fs::exists(&target)).then_some(target)
}

fn filtered_file_dialog(extensions: &[&str]) -> rfd::FileDialog {
    let mut dialog = rfd::FileDialog::new();
    if !extensions.is_empty() {
        dialog = dialog.add_filter("Files", extensions);
    }
    dialog
}

pub(crate) fn pick_existing_file(extensions: &[&str]) -> Option<PathBuf> {
    filtered_file_dialog(extensions).pick_file()
}

pub(crate) fn pick_existing_files(extensions: &[&str]) -> Option<Vec<PathBuf>> {
    normalize_file_selection(filtered_file_dialog(extensions).pick_files()?)
}

fn normalize_file_selection(mut paths: Vec<PathBuf>) -> Option<Vec<PathBuf>> {
    paths.sort();
    paths.dedup();
    if paths.is_empty() { None } else { Some(paths) }
}

pub(crate) fn pick_new_file(extensions: &[&str]) -> Option<PathBuf> {
    filtered_file_dialog(extensions).save_file()
}

pub(crate) fn pick_dir() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_folder()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use common::{TempDir, fs};

    use crate::gui::dialogs::{normalize_file_selection, unconfirmed_target};

    /// Only an extended name that exists and differs from the pick is owed a
    /// second confirmation: a bare name over nothing, and a pick that already
    /// carries the extension (the dialog asked about that very file), are not.
    #[test]
    fn only_an_existing_extended_name_needs_confirming_again() {
        let dir = TempDir::new("darkroom-save-target");
        let bare = dir.join("scene");
        let extended = dir.join("scene.darkroom");
        assert_eq!(unconfirmed_target(&bare), None, "nothing to replace yet");

        fs::write(&extended, b"x").unwrap();
        assert_eq!(unconfirmed_target(&bare), Some(extended.clone()));
        assert_eq!(
            unconfirmed_target(&extended),
            None,
            "the dialog confirmed this file itself"
        );
        assert_eq!(
            unconfirmed_target(&dir.join("scene.DARKROOM")),
            None,
            "the extension matches without regard to case, so nothing is appended"
        );
    }

    #[test]
    fn file_selection_is_sorted_deduplicated_and_nonempty() {
        assert_eq!(normalize_file_selection(Vec::new()), None);
        assert_eq!(
            normalize_file_selection(vec![
                PathBuf::from("b.fit"),
                PathBuf::from("a.fit"),
                PathBuf::from("b.fit"),
            ]),
            Some(vec![PathBuf::from("a.fit"), PathBuf::from("b.fit")])
        );
    }
}
