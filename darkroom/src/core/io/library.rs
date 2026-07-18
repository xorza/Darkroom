//! The shared graph library: graphs that live in the runtime `Library`
//! (where `GraphLink::Shared` resolves) rather than in
//! any one document, so they're reusable across documents. Persisted in
//! the working dir as JSON — loaded into
//! `library.graphs` at startup, saved when "promote" grows it. Failures
//! surface through the caller (`Engine`) into the status log.
//!
//! A file that fails to load still holds graphs a save would destroy (the
//! session degraded to an empty library in its place), so [`load_library`]
//! moves it aside to `<name>.broken` for recovery, and [`save_library`]
//! refuses to overwrite anything it can't re-read — the backstop for a
//! failed move or corruption after startup.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use common::{SerdeFormat, deserialize, serialize};
use scenarium::{Graph, GraphId};

use crate::core::io::cwd_file;

/// Library file name, resolved relative to the process working directory.
const LIBRARY_FILE: &str = "darkroom.library.json";

/// On-disk wrapper so the file is a named table rather than a bare
/// collection (more robust to hand-edit and future fields). A plain
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct LibraryFile {
    #[serde(default)]
    graphs: HashMap<GraphId, Graph>,
}

fn path() -> PathBuf {
    cwd_file(LIBRARY_FILE)
}

/// Where a failed-to-load library file is moved so a later save can't
/// destroy it: `darkroom.library.json` → `darkroom.library.json.broken`.
fn broken_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .expect("library path has a file name")
        .to_os_string();
    name.push(".broken");
    path.with_file_name(name)
}

/// Read + parse the library file, structurally checking every graph — the
/// file is hand-editable, so it's untrusted input gated like a document.
/// A missing file is the normal first-launch case (`Ok(empty)`); any
/// other failure is `Err` (render with `{:#}` for the full reason). No
/// side effects — the recovery move is [`load_library`]'s.
fn read_graphs(path: &Path) -> Result<HashMap<GraphId, Graph>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(err) => return Err(err).with_context(|| path.display().to_string()),
    };
    let lib = deserialize::<LibraryFile>(&bytes, SerdeFormat::Json)
        .with_context(|| path.display().to_string())?;
    for (graph_id, graph) in &lib.graphs {
        anyhow::ensure!(!graph_id.is_nil(), "{}: nil graph id", path.display());
        graph
            .check()
            .with_context(|| format!("{}: invalid graph {:?}", path.display(), graph.name))?;
    }
    Ok(lib.graphs)
}

/// Load the shared graphs from the working dir. On a failed load the
/// session degrades to an empty library, so the file is moved aside to
/// `<name>.broken` — recoverable by the user, and no longer in the way of
/// the saves that would otherwise overwrite it with that empty set. The
/// `Err` carries the reason plus where the file went.
pub(crate) fn load_library() -> Result<HashMap<GraphId, Graph>> {
    load_library_from(&path())
}

fn load_library_from(path: &Path) -> Result<HashMap<GraphId, Graph>> {
    read_graphs(path).map_err(|err| {
        let broken = broken_path(path);
        // Flatten the chain (`:#`) so the recovery note reads after the
        // reason, not as an outer context in front of it.
        match std::fs::rename(path, &broken) {
            Ok(()) => anyhow!("{err:#} — file moved to {}", broken.display()),
            Err(rename_err) => {
                anyhow!("{err:#} — couldn't move the file aside ({rename_err}); fix or remove it")
            }
        }
    })
}

/// Write the shared graphs to the working dir, refusing to overwrite
/// a file whose graphs can't be re-read. The caller
/// (`Engine::edit_library`) reports the error to the status log, which also
/// traces it.
pub(crate) fn save_library<'a>(
    graphs: impl Iterator<Item = (&'a GraphId, &'a Graph)>,
) -> Result<()> {
    save_library_to(&path(), graphs)
}

fn save_library_to<'a>(
    path: &Path,
    graphs: impl Iterator<Item = (&'a GraphId, &'a Graph)>,
) -> Result<()> {
    // Normally a bad file was already moved aside at load; this closes the
    // gaps (a failed move, corruption after startup).
    if let Err(err) = read_graphs(path) {
        bail!("refusing to overwrite a library file that can't be re-read — {err:#}");
    }
    let lib = LibraryFile {
        graphs: graphs.map(|(id, graph)| (*id, graph.clone())).collect(),
    };
    let bytes = serialize(&lib, SerdeFormat::Json)?;
    std::fs::write(path, &bytes).with_context(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use common::test_utils::test_output_path;
    use scenarium::NodeId;
    use scenarium::{GraphEvent, GraphId};

    use super::*;

    fn graph(name: &str) -> Graph {
        Graph::new(name).category("test")
    }

    fn graphs<const N: usize>(names: [&str; N]) -> HashMap<GraphId, Graph> {
        names
            .into_iter()
            .map(|name| (GraphId::unique(), graph(name)))
            .collect()
    }

    #[test]
    fn save_load_roundtrip() {
        let path = test_output_path("darkroom_library/roundtrip.json");
        // A stale file from an earlier run would trip save's re-read guard.
        let _ = std::fs::remove_file(&path);
        let graphs = graphs(["blur", "sharpen"]);
        save_library_to(&path, graphs.iter()).unwrap();

        assert_eq!(load_library_from(&path).unwrap(), graphs);
    }

    #[test]
    fn missing_file_is_empty_and_not_an_error() {
        let path = test_output_path("darkroom_library/never-written.json");
        assert_eq!(load_library_from(&path).unwrap(), HashMap::new());
    }

    #[test]
    fn corrupt_file_is_quarantined_and_the_slot_reusable() {
        let path = test_output_path("darkroom_library/corrupt.json");
        let broken = broken_path(&path);
        let garbage = r#"{"graphs": [ truncated"#;
        std::fs::write(&path, garbage).unwrap();

        let err = format!("{:#}", load_library_from(&path).unwrap_err());
        assert!(
            err.contains(path.to_str().unwrap()) && err.contains(broken.to_str().unwrap()),
            "error names the file and the backup: {err}"
        );
        assert!(!path.exists(), "the corrupt file was moved aside");
        assert_eq!(
            std::fs::read_to_string(&broken).unwrap(),
            garbage,
            "the backup preserves the corrupt content byte-for-byte"
        );

        // With the bad file out of the way, the save→load cycle works again
        // and the backup is untouched.
        let graphs = graphs(["recovered"]);
        save_library_to(&path, graphs.iter()).unwrap();
        assert_eq!(load_library_from(&path).unwrap(), graphs);
        assert_eq!(std::fs::read_to_string(&broken).unwrap(), garbage);
    }

    #[test]
    fn structurally_invalid_def_is_quarantined_like_a_parse_failure() {
        // Parses fine but fails `Graph::check` (dangling event
        // emitter): the load refuses it and moves the file aside, exactly
        // like syntactic corruption.
        let path = test_output_path("darkroom_library/invalid-def.json");
        // A stale file from an earlier run would trip save's re-read guard.
        let _ = std::fs::remove_file(&path);
        let mut bad = graph("dangling");
        bad.events.push(GraphEvent {
            name: "tick".into(),
            emitter: NodeId::unique(),
            emitter_event_idx: 0,
        });
        let graphs = HashMap::from([(GraphId::unique(), bad)]);
        save_library_to(&path, graphs.iter()).unwrap();

        let err = format!("{:#}", load_library_from(&path).unwrap_err());
        assert!(
            err.contains("invalid graph") && err.contains("dangling"),
            "error names the bad graph: {err}"
        );
        assert!(!path.exists(), "the invalid file was moved aside");
    }

    #[test]
    fn save_refuses_to_overwrite_an_unreadable_file() {
        let path = test_output_path("darkroom_library/corrupt-at-save.json");
        let garbage = "not a library";
        std::fs::write(&path, garbage).unwrap();

        let graphs = graphs(["x"]);
        let err = format!("{:#}", save_library_to(&path, graphs.iter()).unwrap_err());
        assert!(
            err.contains(path.to_str().unwrap()),
            "error names the file: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            garbage,
            "the refused save left the file untouched"
        );
    }

    #[test]
    fn unwritable_path_reports_save_failure() {
        let path = test_output_path("darkroom_library/no-such-dir").join("lib.json");
        let graphs = graphs(["x"]);
        let err = format!("{:#}", save_library_to(&path, graphs.iter()).unwrap_err());
        assert!(
            err.contains(path.to_str().unwrap()),
            "error names the file: {err}"
        );
    }
}
