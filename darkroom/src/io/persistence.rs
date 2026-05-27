//! On-disk I/O for darkroom: file-picker dialogs plus the byte
//! ⇄ type plumbing for documents and themes. Pure persistence — no
//! `App` state, no undo stack, no config. `App` orchestrates (when to
//! load/save, what to do with the result); this module only turns
//! paths into values and values into files. Failures log to stderr and
//! return `None`/`false` so a bad read/write degrades instead of
//! crashing the session.

use std::path::{Path, PathBuf};

use common::{SerdeFormat, deserialize, serialize};
use scenarium::data::{FsPathConfig, FsPathMode};
use scenarium::prelude::SubgraphDef;

use crate::document::Document;
use crate::theme::Theme;

/// File-dialog extension filters. First entry is the default — Rhai
/// is the canonical on-disk format for scenarium graphs (matches the
/// deprecated-darkroom file menu's filter list).
const FILE_FILTERS: &[(&str, &[&str])] = &[
    ("Rhai", &["rhai"]),
    ("JSON", &["json"]),
    ("Lz4 compressed Rhai", &["lz4"]),
    ("TOML", &["toml"]),
];

/// Build an `rfd::FileDialog` preconfigured with the project's
/// extension filters and (optionally) a starting directory taken from
/// the last opened/saved path's parent. Shared by the open and save
/// flows so both surfaces stay in sync.
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

pub fn pick_open_path(start: Option<&Path>) -> Option<PathBuf> {
    file_dialog(start).pick_file()
}

pub fn pick_save_path(start: Option<&Path>) -> Option<PathBuf> {
    file_dialog(start).save_file()
}

/// Open a file/folder dialog for an `FsPath` input, honoring its config:
/// the mode picks open-file / save-file / pick-folder, and any extensions
/// become a filter. Returns the chosen path, or `None` if cancelled.
pub fn pick_path(config: &FsPathConfig) -> Option<PathBuf> {
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

/// TOML-only dialog for theme files. Themes always round-trip through
/// TOML (the format the config references by name), so there's no
/// multi-format picker like documents have.
fn theme_dialog() -> rfd::FileDialog {
    rfd::FileDialog::new().add_filter("TOML theme", &["toml"])
}

pub fn pick_theme_open() -> Option<PathBuf> {
    theme_dialog().pick_file()
}

pub fn pick_theme_save() -> Option<PathBuf> {
    theme_dialog().save_file()
}

/// Read + deserialize a document from `path`, picking the format from
/// its extension. Returns `None` (and logs) on an unsupported
/// extension, an unreadable file, or a parse failure.
pub fn load_document(path: &Path) -> Option<Document> {
    let format = match SerdeFormat::from_file_name(&path.to_string_lossy()) {
        Ok(f) => f,
        Err(err) => {
            eprintln!("load failed: unsupported file extension ({err})");
            return None;
        }
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(err) => {
            eprintln!("load failed: {} {err}", path.display());
            return None;
        }
    };
    match Document::deserialize(format, &bytes) {
        Ok(doc) => Some(doc),
        Err(err) => {
            eprintln!("load failed: {} {err}", path.display());
            None
        }
    }
}

/// Serialize `doc` and write it to `path`, picking the format from its
/// extension (defaulting to Rhai for unknown extensions). Returns
/// whether the write succeeded.
pub fn save_document(doc: &Document, path: &Path) -> bool {
    let format = SerdeFormat::from_file_name(&path.to_string_lossy()).unwrap_or(SerdeFormat::Rhai);
    let bytes = doc.serialize(format);
    match std::fs::write(path, &bytes) {
        Ok(()) => true,
        Err(err) => {
            eprintln!("save failed: {} {err}", path.display());
            false
        }
    }
}

/// Read + deserialize a theme `.toml` from `path`. Returns `None` (and
/// logs) on an unreadable file or a parse failure.
pub fn load_theme(path: &Path) -> Option<Theme> {
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

/// Serialize a subgraph `def` and write it to `path`, picking the format
/// from its extension (defaulting to Rhai). The def's interior `Graph`
/// carries its own nested subgraph table, so nested defs travel along
/// automatically. Returns whether the write succeeded.
pub fn export_subgraph(def: &SubgraphDef, path: &Path) -> bool {
    let format = SerdeFormat::from_file_name(&path.to_string_lossy()).unwrap_or(SerdeFormat::Rhai);
    let bytes = serialize(def, format);
    match std::fs::write(path, &bytes) {
        Ok(()) => true,
        Err(err) => {
            eprintln!("subgraph export failed: {} {err}", path.display());
            false
        }
    }
}

/// Read + deserialize a subgraph def from `path`. Returns `None` (and
/// logs) on an unsupported extension, an unreadable file, or a parse
/// failure.
pub fn import_subgraph(path: &Path) -> Option<SubgraphDef> {
    let format = match SerdeFormat::from_file_name(&path.to_string_lossy()) {
        Ok(f) => f,
        Err(err) => {
            eprintln!("subgraph import failed: unsupported file extension ({err})");
            return None;
        }
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(err) => {
            eprintln!("subgraph import failed: {} {err}", path.display());
            return None;
        }
    };
    match deserialize::<SubgraphDef>(&bytes, format) {
        Ok(def) => Some(def),
        Err(err) => {
            eprintln!("subgraph import failed: {} {err}", path.display());
            None
        }
    }
}

/// Serialize `theme` to TOML and write it to `path`.
pub fn export_theme(theme: &Theme, path: &Path) {
    let bytes = serialize(theme, SerdeFormat::Toml);
    if let Err(err) = std::fs::write(path, &bytes) {
        eprintln!("theme export failed: {} {err}", path.display());
    }
}
