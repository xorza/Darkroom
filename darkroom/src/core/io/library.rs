//! The shared graph library: graphs that live in the runtime `Library`
//! (where `GraphLink::Shared` resolves) rather than in
//! any one document, so they're reusable across documents. Persisted in
//! the working dir as JSON — loaded into
//! `library.graphs` at startup, saved when "promote" grows it. Failures
//! surface through `RuntimeLibrary` into the runtime host's status log.
//!
//! A file that fails to load still holds graphs a save would destroy (the
//! session degraded to an empty library in its place), so [`load_library`]
//! moves it aside to `<name>.broken` for recovery, and [`save_library`]
//! refuses to overwrite anything it can't re-read — the backstop for a
//! failed move or corruption after startup.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use common::{DeserializeError, SerdeFormat, SerializeError, deserialize, file_utils, serialize};
use scenarium::{Graph, GraphId};

use crate::core::io::cwd_file;

/// Library file name, resolved relative to the process working directory.
const LIBRARY_FILE: &str = "darkroom.library.json";

/// On-disk wrapper so the file is a named table rather than a bare
/// collection (more robust to hand-edit and future fields). A plain
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedLibrary {
    #[serde(default)]
    pub(crate) graphs: HashMap<GraphId, Graph>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LibraryReadError {
    #[error("{path}: {source}", path = .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {source}", path = .path.display())]
    Deserialize {
        path: PathBuf,
        #[source]
        source: DeserializeError,
    },
    #[error("{path}: nil graph id", path = .path.display())]
    NilGraphId { path: PathBuf },
    #[error("{path}: invalid graph {graph_name:?}: {reason}", path = .path.display())]
    InvalidGraph {
        path: PathBuf,
        graph_name: String,
        reason: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LibraryLoadError {
    #[error("{source} — file moved to {broken_path}", broken_path = .broken_path.display())]
    Quarantined {
        #[source]
        source: Box<LibraryReadError>,
        broken_path: PathBuf,
    },
    #[error("{source} — couldn't move the file aside ({quarantine_error}); fix or remove it")]
    QuarantineFailed {
        #[source]
        source: Box<LibraryReadError>,
        quarantine_error: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LibrarySaveError {
    #[error("refusing to overwrite a library file that can't be re-read — {source}")]
    Unreadable {
        #[source]
        source: Box<LibraryReadError>,
    },
    #[error("failed to serialize library: {source}")]
    Serialize {
        #[source]
        source: SerializeError,
    },
    #[error("{path}: {source}", path = .path.display())]
    Publish {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
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
fn read_library(path: &Path) -> Result<PersistedLibrary, LibraryReadError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(PersistedLibrary::default()),
        Err(source) => {
            return Err(LibraryReadError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let library = deserialize::<PersistedLibrary>(&bytes, SerdeFormat::Json).map_err(|source| {
        LibraryReadError::Deserialize {
            path: path.to_path_buf(),
            source,
        }
    })?;
    for (graph_id, graph) in &library.graphs {
        if graph_id.is_nil() {
            return Err(LibraryReadError::NilGraphId {
                path: path.to_path_buf(),
            });
        }
        graph
            .validate()
            .map_err(|error| LibraryReadError::InvalidGraph {
                path: path.to_path_buf(),
                graph_name: graph.name.clone(),
                reason: format!("{error:#}"),
            })?;
    }
    Ok(library)
}

/// Load the shared graphs from the working dir. On a failed load the
/// session degrades to an empty library, so the file is moved aside to
/// `<name>.broken` — recoverable by the user, and no longer in the way of
/// the saves that would otherwise overwrite it with that empty set. The
/// `Err` carries the reason plus where the file went.
pub(crate) fn load_library() -> Result<PersistedLibrary, LibraryLoadError> {
    load_library_from(&path())
}

fn load_library_from(path: &Path) -> Result<PersistedLibrary, LibraryLoadError> {
    read_library(path).map_err(|err| {
        let broken = broken_path(path);
        match std::fs::rename(path, &broken) {
            Ok(()) => LibraryLoadError::Quarantined {
                source: Box::new(err),
                broken_path: broken,
            },
            Err(quarantine_error) => LibraryLoadError::QuarantineFailed {
                source: Box::new(err),
                quarantine_error,
            },
        }
    })
}

/// Write the shared graphs to the working dir, refusing to overwrite
/// a file whose graphs can't be re-read. The runtime host reports errors
/// returned by `RuntimeLibrary::edit_shared_graphs` to the status log, which also
/// traces it.
pub(crate) fn save_library<'a>(
    graphs: impl Iterator<Item = (&'a GraphId, &'a Graph)>,
) -> Result<(), LibrarySaveError> {
    save_library_to(&path(), graphs)
}

fn save_library_to<'a>(
    path: &Path,
    graphs: impl Iterator<Item = (&'a GraphId, &'a Graph)>,
) -> Result<(), LibrarySaveError> {
    // Normally a bad file was already moved aside at load; this closes the
    // gaps (a failed move, corruption after startup).
    read_library(path).map_err(|source| LibrarySaveError::Unreadable {
        source: Box::new(source),
    })?;
    let library = PersistedLibrary {
        graphs: graphs.map(|(id, graph)| (*id, graph.clone())).collect(),
    };
    let bytes = serialize(&library, SerdeFormat::Json)
        .map_err(|source| LibrarySaveError::Serialize { source })?;
    file_utils::publish_bytes(path, &bytes, file_utils::PublicationMode::Durable).map_err(
        |source| LibrarySaveError::Publish {
            path: path.to_path_buf(),
            source,
        },
    )
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

        assert_eq!(load_library_from(&path).unwrap().graphs, graphs);
    }

    #[test]
    fn missing_file_is_empty_and_not_an_error() {
        let path = test_output_path("darkroom_library/never-written.json");
        assert!(load_library_from(&path).unwrap().graphs.is_empty());
    }

    #[test]
    fn corrupt_file_is_quarantined_and_the_slot_reusable() {
        let path = test_output_path("darkroom_library/corrupt.json");
        let broken = broken_path(&path);
        let garbage = r#"{"graphs": [ truncated"#;
        std::fs::write(&path, garbage).unwrap();

        let error = load_library_from(&path).unwrap_err();
        assert!(
            matches!(
                &error,
                LibraryLoadError::Quarantined { source, broken_path }
                    if matches!(
                        source.as_ref(),
                        LibraryReadError::Deserialize { path: error_path, .. }
                            if error_path == &path
                    ) && broken_path == &broken
            ),
            "parse failure is quarantined with both exact paths: {error}"
        );
        let err = error.to_string();
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
        assert_eq!(load_library_from(&path).unwrap().graphs, graphs);
        assert_eq!(std::fs::read_to_string(&broken).unwrap(), garbage);
    }

    #[test]
    fn structurally_invalid_def_is_quarantined_like_a_parse_failure() {
        // Parses fine but fails `Graph::validate` (dangling event
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

        let error = load_library_from(&path).unwrap_err();
        assert!(
            matches!(
                &error,
                LibraryLoadError::Quarantined { source, .. }
                    if matches!(
                        source.as_ref(),
                        LibraryReadError::InvalidGraph {
                            path: error_path,
                            graph_name,
                            ..
                        } if error_path == &path && graph_name == "dangling"
                    )
            ),
            "structural failure retains its typed context: {error}"
        );
        let err = error.to_string();
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
        if path.parent().unwrap().exists() {
            std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        }
        let graphs = graphs(["x"]);
        let err = format!("{:#}", save_library_to(&path, graphs.iter()).unwrap_err());
        assert!(
            err.contains(path.to_str().unwrap()),
            "error names the file: {err}"
        );
    }
}
