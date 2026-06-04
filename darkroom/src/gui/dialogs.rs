//! File-picker dialogs (rfd) and theme file I/O — the GUI-only half of
//! on-disk I/O. The document/subgraph byte⇄type plumbing lives in
//! `crate::core::io::persistence` (no GUI deps); this side opens native dialogs
//! and round-trips the GUI `Theme`. Failures log to stderr and return
//! `None` so a cancelled/failed pick degrades instead of crashing.

use std::path::{Path, PathBuf};

use common::{SerdeFormat, deserialize, serialize};
use scenarium::data::{FsPathConfig, FsPathMode};

use crate::gui::theme::Theme;

/// File-dialog extension filters. First entry is the default — Rhai is the
/// canonical on-disk format for scenarium graphs.
const FILE_FILTERS: &[(&str, &[&str])] = &[
    ("Rhai", &["rhai"]),
    ("JSON", &["json"]),
    ("Lz4 compressed Rhai", &["lz4"]),
    ("TOML", &["toml"]),
];

/// Build an `rfd::FileDialog` preconfigured with the project's extension
/// filters and (optionally) a starting directory from the last path's
/// parent. Shared by the open and save flows so both stay in sync.
fn file_dialog(start: Option<&Path>) -> rfd::FileDialog {
    let mut dialog = rfd::FileDialog::new();
    for (name, exts) in FILE_FILTERS {
        dialog = dialog.add_filter(*name, exts);
    }
    if let Some(parent) = start.and_then(Path::parent) {
        dialog = dialog.set_directory(parent);
    }
    dialog
}

pub(crate) fn pick_open_path(start: Option<&Path>) -> Option<PathBuf> {
    file_dialog(start).pick_file()
}

pub(crate) fn pick_save_path(start: Option<&Path>) -> Option<PathBuf> {
    file_dialog(start).save_file()
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

/// TOML-only dialog for theme files. Themes always round-trip through TOML
/// (the format the config references by name), so there's no multi-format
/// picker like documents have.
fn theme_dialog() -> rfd::FileDialog {
    rfd::FileDialog::new().add_filter("TOML theme", &["toml"])
}

pub(crate) fn pick_theme_open() -> Option<PathBuf> {
    theme_dialog().pick_file()
}

pub(crate) fn pick_theme_save() -> Option<PathBuf> {
    theme_dialog().save_file()
}

/// Read + deserialize a theme `.toml` from `path`. Returns `None` (and
/// logs) on an unreadable file or a parse failure.
pub(crate) fn load_theme(path: &Path) -> Option<Theme> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(err) => {
            eprintln!("theme load failed: {} {err}", path.display());
            return None;
        }
    };
    match deserialize::<Theme>(&bytes, SerdeFormat::Toml) {
        Ok(theme) => Some(theme),
        Err(err) => {
            eprintln!("theme load failed: {} {err}", path.display());
            None
        }
    }
}

/// Serialize `theme` to TOML and write it to `path`.
pub(crate) fn export_theme(theme: &Theme, path: &Path) {
    let bytes = match serialize(theme, SerdeFormat::Toml) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("theme export failed: {} {err}", path.display());
            return;
        }
    };
    if let Err(err) = std::fs::write(path, &bytes) {
        eprintln!("theme export failed: {} {err}", path.display());
    }
}
