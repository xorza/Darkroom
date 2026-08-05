//! Native file-picker dialogs (rfd). Project byte⇄type plumbing lives in
//! `crate::core::io::document`; this side hands paths off to that GUI-free
//! module. Failures degrade — a cancelled or failed pick returns `None` —
//! rather than crashing. Everything else the shell does for us is in
//! `crate::platform`.

use std::path::{Path, PathBuf};

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

pub(crate) fn pick_project_save_path(start: Option<&Path>) -> Option<PathBuf> {
    file_dialog(start)
        .add_filter("Darkroom project", &[document::EXTENSION])
        .save_file()
        .map(document::with_extension)
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

pub(crate) fn pick_directory() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_folder()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::gui::dialogs::normalize_file_selection;

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
