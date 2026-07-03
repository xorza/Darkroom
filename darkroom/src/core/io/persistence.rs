//! Byte ⇄ type plumbing for documents and subgraphs — the GUI-free half
//! of on-disk I/O. The file-picker dialogs and theme I/O live in
//! `crate::gui::dialogs`. Pure persistence — no app state, no undo stack,
//! no config; callers orchestrate (when to load/save, what to do with the
//! result), this only turns paths into values and values into files.
//! Failures log to stderr and return `None`/`false` so a bad read/write
//! degrades instead of crashing the session.

use std::path::Path;

use common::{SerdeFormat, deserialize, serialize};
use scenarium::graph::subgraph::SubgraphDef;

use crate::core::document::Document;

/// Read + deserialize a document from `path`, picking the format from its
/// extension. Returns `None` (and logs) on an unsupported extension, an
/// unreadable file, or a parse failure.
pub(crate) fn load_document(path: &Path) -> Option<Document> {
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
/// extension (defaulting to Rhai for unknown extensions). Returns whether
/// the write succeeded.
pub(crate) fn save_document(doc: &Document, path: &Path) -> bool {
    let format = SerdeFormat::from_file_name(&path.to_string_lossy()).unwrap_or(SerdeFormat::Rhai);
    let bytes = match doc.serialize(format) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("save failed: {} {err}", path.display());
            return false;
        }
    };
    match std::fs::write(path, &bytes) {
        Ok(()) => true,
        Err(err) => {
            eprintln!("save failed: {} {err}", path.display());
            false
        }
    }
}

/// Serialize a subgraph `def` and write it to `path`, picking the format
/// from its extension (defaulting to Rhai). The def's interior `Graph`
/// carries its own nested subgraph table, so nested defs travel along
/// automatically. Returns whether the write succeeded.
pub(crate) fn export_subgraph(def: &SubgraphDef, path: &Path) -> bool {
    let format = SerdeFormat::from_file_name(&path.to_string_lossy()).unwrap_or(SerdeFormat::Rhai);
    let bytes = match serialize(def, format) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("subgraph export failed: {} {err}", path.display());
            return false;
        }
    };
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
pub(crate) fn import_subgraph(path: &Path) -> Option<SubgraphDef> {
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
