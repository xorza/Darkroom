//! GUI-side OS shell integration: native file-picker dialogs (rfd) and
//! opening URLs in the user's browser. Project and reusable-graph byte⇄type
//! plumbing live in `crate::core::io::{document, graph_template}`; this side hands
//! paths off to those GUI-free modules. Failures degrade — a cancelled/failed
//! pick returns `None`, a failed URL open logs — rather than crashing.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::core::io::document;

const GRAPH_FILE_FILTERS: &[(&str, &[&str])] = &[
    ("JSON", &["json"]),
    ("Lz4 compressed JSON", &["lz4"]),
    ("Bitcode", &["bin"]),
    ("TOML", &["toml"]),
];

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

pub(crate) fn pick_graph_template_open_path(start: Option<&Path>) -> Option<PathBuf> {
    graph_file_dialog(start).pick_file()
}

pub(crate) fn pick_graph_template_save_path(start: Option<&Path>) -> Option<PathBuf> {
    graph_file_dialog(start).save_file()
}

fn graph_file_dialog(start: Option<&Path>) -> rfd::FileDialog {
    let mut dialog = file_dialog(start);
    for (name, extensions) in GRAPH_FILE_FILTERS {
        dialog = dialog.add_filter(*name, extensions);
    }
    dialog
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

/// Open `url` in the user's default browser via the platform opener. Non-
/// blocking (the spawn returns immediately, so it's safe to call mid-record,
/// unlike the modal file dialogs). A missing opener just logs and degrades —
/// there's no user-facing error surface yet.
pub(crate) fn open_url(url: &str) {
    #[cfg(target_os = "linux")]
    let mut cmd = Command::new("xdg-open");
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        // `start` treats its first quoted arg as the window title, so pass an
        // empty title before the URL.
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    if let Err(e) = cmd.arg(url).spawn() {
        tracing::warn!("failed to open url {url}: {e}");
    }
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
