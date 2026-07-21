//! Import and export of reusable graph-template files.

use std::path::{Path, PathBuf};

use common::{
    DeserializeError, FileExtensionError, SerdeFormat, SerializeError, deserialize, file_utils,
    serialize,
};
use scenarium::Graph;

#[derive(Debug, thiserror::Error)]
pub(crate) enum GraphTemplateLoadError {
    #[error("{path}: {source}", path = .path.display())]
    Format {
        path: PathBuf,
        #[source]
        source: FileExtensionError,
    },
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
    #[error("{path}: invalid graph template: {reason}", path = .path.display())]
    Invalid { path: PathBuf, reason: String },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum GraphTemplateSaveError {
    #[error("failed to serialize graph template: {source}")]
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

pub(crate) fn save(graph: &Graph, path: &Path) -> Result<(), GraphTemplateSaveError> {
    let format = SerdeFormat::from_file_name(&path.to_string_lossy()).unwrap_or(SerdeFormat::Json);
    let bytes =
        serialize(graph, format).map_err(|source| GraphTemplateSaveError::Serialize { source })?;
    file_utils::publish_bytes(path, &bytes, file_utils::PublicationMode::Durable).map_err(
        |source| GraphTemplateSaveError::Publish {
            path: path.to_path_buf(),
            source,
        },
    )
}

pub(crate) fn load(path: &Path) -> Result<Graph, GraphTemplateLoadError> {
    let format = SerdeFormat::from_file_name(&path.to_string_lossy()).map_err(|source| {
        GraphTemplateLoadError::Format {
            path: path.to_path_buf(),
            source,
        }
    })?;
    let bytes = std::fs::read(path).map_err(|source| GraphTemplateLoadError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let graph = deserialize::<Graph>(&bytes, format).map_err(|source| {
        GraphTemplateLoadError::Deserialize {
            path: path.to_path_buf(),
            source,
        }
    })?;
    graph
        .validate()
        .map_err(|error| GraphTemplateLoadError::Invalid {
            path: path.to_path_buf(),
            reason: format!("{error:#}"),
        })?;
    Ok(graph)
}

#[cfg(test)]
mod tests;
