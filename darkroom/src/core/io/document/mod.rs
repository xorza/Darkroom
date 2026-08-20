//! Darkroom document archives: one validated JSON document inside a ZIP file.

use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use common::file_utils;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::core::document::Document;
use crate::core::document::error::DocumentValidationError;

pub(crate) const EXTENSION: &str = "darkroom";
const DOCUMENT_ENTRY: &str = "document.json";
const MAX_DOCUMENT_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub(crate) enum DocumentLoadError {
    #[error("{path} must use the .darkroom extension", path = .path.display())]
    InvalidExtension { path: PathBuf },
    #[error("{path}: {source}", path = .path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} is not a valid Darkroom archive: {source}", path = .path.display())]
    InvalidArchive {
        path: PathBuf,
        #[source]
        source: zip::result::ZipError,
    },
    #[error("failed to inspect {path}: {source}", path = .path.display())]
    InspectArchive {
        path: PathBuf,
        #[source]
        source: zip::result::ZipError,
    },
    #[error("{path} contains overlapping ZIP entries", path = .path.display())]
    OverlappingEntries { path: PathBuf },
    #[error("{path} must contain exactly one document.json, found {count}", path = .path.display())]
    DocumentEntryCount { path: PathBuf, count: usize },
    #[error("failed to open document.json in {path}: {source}", path = .path.display())]
    OpenDocumentEntry {
        path: PathBuf,
        #[source]
        source: zip::result::ZipError,
    },
    #[error("{path} contains a non-file document.json entry", path = .path.display())]
    NonFileDocumentEntry { path: PathBuf },
    #[error(
        "document.json in {path} is {size} bytes, exceeding the {max_mib} MiB size limit",
        path = .path.display(),
        max_mib = MAX_DOCUMENT_BYTES / (1024 * 1024)
    )]
    DocumentTooLarge { path: PathBuf, size: u64 },
    #[error("failed to read document.json from {path}: {source}", path = .path.display())]
    ReadDocument {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid document.json in {path}: {source}", path = .path.display())]
    DeserializeDocument {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("{path}: {source}", path = .path.display())]
    InvalidDocument {
        path: PathBuf,
        #[source]
        source: DocumentValidationError,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DocumentSaveError {
    #[error("{path} must use the .{EXTENSION} extension", path = .path.display())]
    InvalidExtension { path: PathBuf },
    #[error("failed to serialize {DOCUMENT_ENTRY} for {path}: {source}", path = .path.display())]
    Serialize {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "{DOCUMENT_ENTRY} for {path} is {size} bytes, exceeding the {max_mib} MiB size limit",
        path = .path.display(),
        max_mib = MAX_DOCUMENT_BYTES / (1024 * 1024)
    )]
    DocumentTooLarge { path: PathBuf, size: u64 },
    /// Refused *before* writing, so a document that the next [`load`]
    /// would reject can never replace the file already on disk. Same
    /// predicate as [`DocumentLoadError::InvalidDocument`] — the two
    /// directions can't drift.
    #[error("{path}: {source}", path = .path.display())]
    InvalidDocument {
        path: PathBuf,
        #[source]
        source: DocumentValidationError,
    },
    #[error("{path}: {source}", path = .path.display())]
    Publish {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub(crate) fn load(path: &Path) -> Result<Document, DocumentLoadError> {
    if !has_extension(path) {
        return Err(DocumentLoadError::InvalidExtension {
            path: path.to_path_buf(),
        });
    }

    let file = File::open(path).map_err(|source| DocumentLoadError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let mut archive =
        ZipArchive::new(file).map_err(|source| DocumentLoadError::InvalidArchive {
            path: path.to_path_buf(),
            source,
        })?;
    let has_overlapping_files =
        archive
            .has_overlapping_files()
            .map_err(|source| DocumentLoadError::InspectArchive {
                path: path.to_path_buf(),
                source,
            })?;
    if has_overlapping_files {
        return Err(DocumentLoadError::OverlappingEntries {
            path: path.to_path_buf(),
        });
    }

    let document_entries = archive
        .file_names()
        .filter(|name| *name == DOCUMENT_ENTRY)
        .count();
    if document_entries != 1 {
        return Err(DocumentLoadError::DocumentEntryCount {
            path: path.to_path_buf(),
            count: document_entries,
        });
    }

    let mut entry =
        archive
            .by_name(DOCUMENT_ENTRY)
            .map_err(|source| DocumentLoadError::OpenDocumentEntry {
                path: path.to_path_buf(),
                source,
            })?;
    if !entry.is_file() {
        return Err(DocumentLoadError::NonFileDocumentEntry {
            path: path.to_path_buf(),
        });
    }
    ensure_load_document_size(path, entry.size())?;

    let mut json = Vec::with_capacity(entry.size() as usize);
    (&mut entry)
        .take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut json)
        .map_err(|source| DocumentLoadError::ReadDocument {
            path: path.to_path_buf(),
            source,
        })?;
    ensure_load_document_size(path, json.len() as u64)?;

    let document: Document =
        serde_json::from_slice(&json).map_err(|source| DocumentLoadError::DeserializeDocument {
            path: path.to_path_buf(),
            source,
        })?;
    document
        .validate()
        .map_err(|source| DocumentLoadError::InvalidDocument {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(document)
}

pub(crate) fn save(document: &Document, path: &Path) -> Result<(), DocumentSaveError> {
    ensure_extension(path)?;
    document
        .validate()
        .map_err(|source| DocumentSaveError::InvalidDocument {
            path: path.to_path_buf(),
            source,
        })?;

    let json =
        serde_json::to_vec_pretty(document).map_err(|source| DocumentSaveError::Serialize {
            path: path.to_path_buf(),
            source,
        })?;
    ensure_save_document_size(path, json.len() as u64)?;

    file_utils::publish(path, file_utils::PublicationMode::Durable, |file| {
        write_archive(file, &json)
    })
    .map_err(|source| DocumentSaveError::Publish {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn with_extension(mut path: PathBuf) -> PathBuf {
    if !has_extension(&path) {
        path.set_extension(EXTENSION);
    }
    path
}

fn ensure_extension(path: &Path) -> Result<(), DocumentSaveError> {
    if has_extension(path) {
        Ok(())
    } else {
        Err(DocumentSaveError::InvalidExtension {
            path: path.to_path_buf(),
        })
    }
}

fn has_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(EXTENSION))
}

fn ensure_save_document_size(path: &Path, size: u64) -> Result<(), DocumentSaveError> {
    if size <= MAX_DOCUMENT_BYTES {
        Ok(())
    } else {
        Err(DocumentSaveError::DocumentTooLarge {
            path: path.to_path_buf(),
            size,
        })
    }
}

fn ensure_load_document_size(path: &Path, size: u64) -> Result<(), DocumentLoadError> {
    if size > MAX_DOCUMENT_BYTES {
        return Err(DocumentLoadError::DocumentTooLarge {
            path: path.to_path_buf(),
            size,
        });
    }
    Ok(())
}

fn write_archive(file: &mut File, json: &[u8]) -> io::Result<()> {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut archive = ZipWriter::new(file);
    archive
        .start_file(DOCUMENT_ENTRY, options)
        .map_err(io::Error::other)?;
    archive.write_all(json)?;
    archive.finish().map_err(io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod tests;
