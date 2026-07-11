//! Byte ⇄ type plumbing for documents and subgraphs — the GUI-free half
//! of on-disk I/O. The file-picker dialogs and theme I/O live in
//! `crate::gui::dialogs`. Pure persistence — no app state, no undo stack,
//! no preferences; callers orchestrate (when to load/save, what to do with the
//! result), this only turns paths into values and values into files.
//! Failures log the detail via `tracing` and return `None`/`false` so a bad
//! read/write degrades instead of crashing; the GUI caller surfaces a short
//! message in the status bar off the return value.

use std::fmt::Display;
use std::path::Path;

use common::{SerdeFormat, deserialize, serialize};
use scenarium::graph::subgraph::SubgraphDef;

use crate::core::document::Document;

/// Read + deserialize a document from `path`, picking the format from its
/// extension. Returns `None` (and logs) on an unsupported extension, an
/// unreadable file, or a parse failure.
pub(crate) fn load_document(path: &Path) -> Option<Document> {
    load_typed(path, "load", |format, bytes| {
        Document::deserialize(format, bytes)
    })
}

/// Serialize `doc` and write it to `path`, picking the format from its
/// extension (defaulting to Rhai for unknown extensions). Returns whether
/// the write succeeded.
pub(crate) fn save_document(doc: &Document, path: &Path) -> bool {
    save_typed(path, "save", |format| doc.serialize(format))
}

/// Serialize a subgraph `def` and write it to `path`, picking the format
/// from its extension (defaulting to Rhai). The def's interior `Graph`
/// carries its own nested subgraph table, so nested defs travel along
/// automatically. Returns whether the write succeeded.
pub(crate) fn export_subgraph(def: &SubgraphDef, path: &Path) -> bool {
    save_typed(path, "subgraph export", |format| serialize(def, format))
}

/// Read + deserialize a subgraph def from `path`. Returns `None` (and
/// logs) on an unsupported extension, an unreadable file, or a parse
/// failure.
pub(crate) fn import_subgraph(path: &Path) -> Option<SubgraphDef> {
    load_typed(path, "subgraph import", |format, bytes| {
        deserialize::<SubgraphDef>(bytes, format)
    })
}

/// Shared load shell: pick the format from the extension, read the file,
/// hand both to `parse`. Every failure logs its detail under `label` and
/// degrades to `None`. `parse` is a closure (not plain `deserialize`)
/// because documents route through the validating
/// [`Document::deserialize`] while subgraphs use bare serde.
fn load_typed<T, E: Display>(
    path: &Path,
    label: &str,
    parse: impl FnOnce(SerdeFormat, &[u8]) -> Result<T, E>,
) -> Option<T> {
    let format = match SerdeFormat::from_file_name(&path.to_string_lossy()) {
        Ok(f) => f,
        Err(err) => {
            tracing::error!("{label} failed: unsupported file extension ({err})");
            return None;
        }
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(err) => {
            tracing::error!("{label} failed: {} {err}", path.display());
            return None;
        }
    };
    match parse(format, &bytes) {
        Ok(value) => Some(value),
        Err(err) => {
            tracing::error!("{label} failed: {} {err}", path.display());
            None
        }
    }
}

/// Shared save shell: pick the format from the extension (Rhai default),
/// encode via `encode`, write the bytes. Every failure logs its detail
/// under `label` and degrades to `false`.
fn save_typed<E: Display>(
    path: &Path,
    label: &str,
    encode: impl FnOnce(SerdeFormat) -> Result<Vec<u8>, E>,
) -> bool {
    let format = SerdeFormat::from_file_name(&path.to_string_lossy()).unwrap_or(SerdeFormat::Rhai);
    let bytes = match encode(format) {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::error!("{label} failed: {} {err}", path.display());
            return false;
        }
    };
    match std::fs::write(path, &bytes) {
        Ok(()) => true,
        Err(err) => {
            tracing::error!("{label} failed: {} {err}", path.display());
            false
        }
    }
}
