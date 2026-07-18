//! Byte ⇄ type plumbing for documents and reusable graphs — the GUI-free half
//! of on-disk I/O. The file-picker dialogs and theme I/O live in
//! `crate::gui::dialogs`. Pure persistence — no app state, no undo stack,
//! no preferences; callers orchestrate (when to load/save, what to do with the
//! result), this only turns paths into values and values into files.
//! Failures return the reason with the path attached (render with `{:#}`);
//! callers surface it through the status log, which also traces it.

use std::path::Path;

use anyhow::{Context, Result};
use common::{SerdeFormat, deserialize, serialize};
use scenarium::Graph;

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

/// Serialize a reusable graph and its nested graphs.
pub(crate) fn export_graph(graph: &Graph, path: &Path) -> Result<()> {
    save_typed(path, |format| serialize(graph, format))
}

/// Read and validate a reusable graph from `path`.
pub(crate) fn import_graph(path: &Path) -> Result<Graph> {
    load_typed(path, |format, bytes| {
        let graph = deserialize::<Graph>(bytes, format)?;
        graph.check()?;
        Ok(graph)
    })
}

/// Shared load shell: pick the format from the extension, read the file,
/// hand both to `parse`. `parse` is a closure (not plain `deserialize`)
/// because each type routes through its validating gate —
/// [`Document::deserialize`] / [`Graph::check`].
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
    use scenarium::GraphEvent;
    use scenarium::NodeId;

    use super::*;

    #[test]
    fn graph_import_round_trips_and_gates_on_check() {
        let path = test_output_path("darkroom_persistence/graph.rhai");
        let graph = Graph::new("ok").category("test");
        export_graph(&graph, &path).unwrap();
        assert_eq!(import_graph(&path).expect("valid graph imports"), graph);

        // A graph that parses but fails `Graph::check` (dangling event
        // emitter) is refused at the import gate. Export doesn't gate —
        // it writes editor-built state.
        let bad_path = test_output_path("darkroom_persistence/bad-graph.rhai");
        let mut bad = Graph::new("bad");
        bad.events.push(GraphEvent {
            name: "tick".into(),
            emitter: NodeId::unique(),
            emitter_event_idx: 0,
        });
        export_graph(&bad, &bad_path).unwrap();
        let err = format!("{:#}", import_graph(&bad_path).unwrap_err());
        assert!(
            err.contains("names missing emitter"),
            "structurally invalid graph must not import: {err}"
        );
    }
}
