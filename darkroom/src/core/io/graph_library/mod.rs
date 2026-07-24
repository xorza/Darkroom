//! Durable storage for [`GraphLibrary`](crate::core::graph_library::GraphLibrary).

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use common::{DeserializeError, SerdeFormat, SerializeError, deserialize, file_utils, serialize};

use crate::core::graph_library::GraphLibrary;
use crate::core::io::cwd_file;

const GRAPH_LIBRARY_FILE: &str = "darkroom.graph-library.json";

#[derive(Debug, thiserror::Error)]
pub(crate) enum GraphLibraryReadError {
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
pub(crate) enum GraphLibraryLoadError {
    #[error("{source} — file moved to {broken_path}", broken_path = .broken_path.display())]
    Quarantined {
        #[source]
        source: Box<GraphLibraryReadError>,
        broken_path: PathBuf,
    },
    #[error("{source} — couldn't move the file aside ({quarantine_error}); fix or remove it")]
    QuarantineFailed {
        #[source]
        source: Box<GraphLibraryReadError>,
        quarantine_error: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum GraphLibrarySaveError {
    #[error("refusing to overwrite a graph-library file that can't be re-read — {source}")]
    Unreadable {
        #[source]
        source: Box<GraphLibraryReadError>,
    },
    #[error("failed to serialize graph library: {source}")]
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
    cwd_file(GRAPH_LIBRARY_FILE)
}

fn broken_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .expect("graph-library path has a file name")
        .to_os_string();
    name.push(".broken");
    path.with_file_name(name)
}

fn read(path: &Path) -> Result<GraphLibrary, GraphLibraryReadError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(GraphLibrary::default()),
        Err(source) => {
            return Err(GraphLibraryReadError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let library = deserialize::<GraphLibrary>(&bytes, SerdeFormat::Json).map_err(|source| {
        GraphLibraryReadError::Deserialize {
            path: path.to_path_buf(),
            source,
        }
    })?;
    for (graph_id, graph) in &library.graphs {
        if graph_id.is_nil() {
            return Err(GraphLibraryReadError::NilGraphId {
                path: path.to_path_buf(),
            });
        }
        graph
            .validate_subgraph()
            .map_err(|error| GraphLibraryReadError::InvalidGraph {
                path: path.to_path_buf(),
                graph_name: graph
                    .definition
                    .as_ref()
                    .map(|definition| definition.name.clone())
                    .unwrap_or_else(|| "<missing definition>".to_owned()),
                reason: format!("{error:#}"),
            })?;
    }
    Ok(library)
}

pub(crate) fn load() -> Result<GraphLibrary, GraphLibraryLoadError> {
    load_from(&path())
}

fn load_from(path: &Path) -> Result<GraphLibrary, GraphLibraryLoadError> {
    read(path).map_err(|error| {
        let broken = broken_path(path);
        match std::fs::rename(path, &broken) {
            Ok(()) => GraphLibraryLoadError::Quarantined {
                source: Box::new(error),
                broken_path: broken,
            },
            Err(quarantine_error) => GraphLibraryLoadError::QuarantineFailed {
                source: Box::new(error),
                quarantine_error,
            },
        }
    })
}

pub(crate) fn save(library: &GraphLibrary) -> Result<(), GraphLibrarySaveError> {
    save_to(&path(), library)
}

fn save_to(path: &Path, library: &GraphLibrary) -> Result<(), GraphLibrarySaveError> {
    read(path).map_err(|source| GraphLibrarySaveError::Unreadable {
        source: Box::new(source),
    })?;
    let bytes = serialize(library, SerdeFormat::Json)
        .map_err(|source| GraphLibrarySaveError::Serialize { source })?;
    file_utils::publish_bytes(path, &bytes, file_utils::PublicationMode::Durable).map_err(
        |source| GraphLibrarySaveError::Publish {
            path: path.to_path_buf(),
            source,
        },
    )
}

#[cfg(test)]
mod tests;
