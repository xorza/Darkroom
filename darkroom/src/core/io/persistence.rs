//! Byte ⇄ type plumbing for documents and subgraphs — the GUI-free half
//! of on-disk I/O. The file-picker dialogs and theme I/O live in
//! `crate::gui::dialogs`. Pure persistence — no app state, no undo stack,
//! no preferences; callers orchestrate (when to load/save, what to do with the
//! result), this only turns paths into values and values into files.
//! Failures return the reason with the path attached (render with `{:#}`);
//! callers surface it through the status log, which also traces it.

use std::path::Path;

use anyhow::{Context, Result};
use common::{SerdeFormat, deserialize, serialize};
use scenarium::graph::subgraph::SubgraphDef;

use crate::core::document::Document;

/// Read + deserialize a document from `path`, picking the format from its
/// extension. Errors on an unsupported extension, an unreadable file, a
/// parse failure, or a document that fails [`Document::check`].
pub(crate) fn load_document(path: &Path) -> Result<Document> {
    load_typed(path, Document::deserialize)
}

/// Serialize `doc` and write it to `path`, picking the format from its
/// extension (defaulting to Rhai for unknown extensions).
pub(crate) fn save_document(doc: &Document, path: &Path) -> Result<()> {
    save_typed(path, |format| doc.serialize(format))
}

/// Serialize a subgraph `def` and write it to `path`, picking the format
/// from its extension (defaulting to Rhai). The def's interior `Graph`
/// carries its own nested subgraph table, so nested defs travel along
/// automatically.
pub(crate) fn export_subgraph(def: &SubgraphDef, path: &Path) -> Result<()> {
    save_typed(path, |format| serialize(def, format))
}

/// Read + deserialize a subgraph def from `path`. Errors on an unsupported
/// extension, an unreadable file, a parse failure, or a def that fails
/// [`SubgraphDef::check`] — an imported file is untrusted input, gated
/// like a document.
pub(crate) fn import_subgraph(path: &Path) -> Result<SubgraphDef> {
    load_typed(path, |format, bytes| {
        let def = deserialize::<SubgraphDef>(bytes, format)?;
        def.check()?;
        Ok(def)
    })
}

/// Shared load shell: pick the format from the extension, read the file,
/// hand both to `parse`. `parse` is a closure (not plain `deserialize`)
/// because each type routes through its validating gate —
/// [`Document::deserialize`] / [`SubgraphDef::check`].
fn load_typed<T>(path: &Path, parse: impl FnOnce(SerdeFormat, &[u8]) -> Result<T>) -> Result<T> {
    let format =
        SerdeFormat::from_file_name(&path.to_string_lossy()).context("unsupported file")?;
    let bytes = std::fs::read(path).with_context(|| path.display().to_string())?;
    parse(format, &bytes).with_context(|| path.display().to_string())
}

/// Shared save shell: pick the format from the extension (Rhai default),
/// encode via `encode`, write the bytes.
fn save_typed(path: &Path, encode: impl FnOnce(SerdeFormat) -> Result<Vec<u8>>) -> Result<()> {
    let format = SerdeFormat::from_file_name(&path.to_string_lossy()).unwrap_or(SerdeFormat::Rhai);
    let bytes = encode(format)?;
    std::fs::write(path, &bytes).with_context(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use common::test_utils::test_output_path;
    use scenarium::graph::NodeId;
    use scenarium::graph::subgraph::{SubgraphEvent, SubgraphId};

    use super::*;

    #[test]
    fn subgraph_import_round_trips_and_gates_on_check() {
        // A well-formed def survives the export → import round trip.
        let path = test_output_path("darkroom_persistence/def.rhai");
        let def = SubgraphDef::new(SubgraphId::unique(), "ok").category("test");
        export_subgraph(&def, &path).unwrap();
        assert_eq!(import_subgraph(&path).expect("valid def imports"), def);

        // A def that parses but fails `SubgraphDef::check` (dangling event
        // emitter) is refused at the import gate. Export doesn't gate —
        // it writes editor-built state.
        let bad_path = test_output_path("darkroom_persistence/bad-def.rhai");
        let mut bad = SubgraphDef::new(SubgraphId::unique(), "bad");
        bad.events.push(SubgraphEvent {
            name: "tick".into(),
            emitter: NodeId::unique(),
            emitter_event_idx: 0,
        });
        export_subgraph(&bad, &bad_path).unwrap();
        let err = format!("{:#}", import_subgraph(&bad_path).unwrap_err());
        assert!(
            err.contains("names missing emitter"),
            "structurally invalid def must not import: {err}"
        );
    }
}
