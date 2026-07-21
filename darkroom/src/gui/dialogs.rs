//! GUI-side OS shell integration: native file-picker dialogs (rfd) and
//! opening URLs in the user's browser. Project and reusable-graph byte⇄type
//! plumbing live in `crate::core::io::{project, persistence}`; this side hands
//! paths off to those GUI-free modules. Failures degrade — a cancelled/failed
//! pick returns `None`, a failed URL open logs — rather than crashing.

use std::path::{Path, PathBuf};
use std::process::Command;

use scenarium::{FsPathConfig, FsPathMode};

use crate::core::io::project;

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
        .add_filter("Darkroom project", &[project::EXTENSION])
        .pick_file()
}

pub(crate) fn pick_project_save_path(start: Option<&Path>) -> Option<PathBuf> {
    file_dialog(start)
        .add_filter("Darkroom project", &[project::EXTENSION])
        .save_file()
        .map(project::with_extension)
}

pub(crate) fn pick_graph_open_path(start: Option<&Path>) -> Option<PathBuf> {
    graph_file_dialog(start).pick_file()
}

pub(crate) fn pick_graph_save_path(start: Option<&Path>) -> Option<PathBuf> {
    graph_file_dialog(start).save_file()
}

fn graph_file_dialog(start: Option<&Path>) -> rfd::FileDialog {
    let mut dialog = file_dialog(start);
    for (name, extensions) in GRAPH_FILE_FILTERS {
        dialog = dialog.add_filter(*name, extensions);
    }
    dialog
}

/// Open a file/folder dialog for an `FsPath` input, honoring its config:
/// the mode picks open-file / save-file / pick-folder, and any extensions
/// become a filter. Returns the chosen path, or `None` if cancelled.
pub(crate) fn pick_path(config: &FsPathConfig) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new();
    if !config.extensions.is_empty() {
        let exts: Vec<&str> = config.extensions.iter().map(String::as_str).collect();
        dialog = dialog.add_filter("Files", &exts);
    }
    match config.mode {
        FsPathMode::ExistingFile => dialog.pick_file(),
        FsPathMode::NewFile => dialog.save_file(),
        FsPathMode::Directory => dialog.pick_folder(),
    }
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
