//! Darkroom document archives: one RON document inside a ZIP file.

pub(crate) mod error;

use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::path::Path;

use common::{SerdeFormat, fs};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::core::document::Document;
use crate::core::io::document::error::{DocumentLoadError, DocumentSaveError};

pub(crate) const EXTENSION: &str = "darkroom";
const DOCUMENT_ENTRY: &str = "document.ron";
const MAX_DOCUMENT_BYTES: u64 = 256 * 1024 * 1024;

/// Decode the document at `path`. What it holds is made consistent by
/// [`Document::reconcile`], which the caller runs against its library.
pub(crate) fn load(path: &Path) -> Result<Document, DocumentLoadError> {
    if !has_extension(path) {
        return Err(DocumentLoadError::InvalidExtension {
            path: path.to_path_buf(),
        });
    }

    let file = fs::open(path).map_err(|source| DocumentLoadError::Open {
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
        .try_fold(0, |count, name| {
            name.map(|name| count + usize::from(name == DOCUMENT_ENTRY))
        })
        .map_err(|source| DocumentLoadError::InspectArchive {
            path: path.to_path_buf(),
            source,
        })?;
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
    if !fits(entry.size()) {
        return Err(DocumentLoadError::DocumentTooLarge {
            path: path.to_path_buf(),
            size: entry.size(),
        });
    }

    let mut encoded = Vec::with_capacity(
        usize::try_from(entry.size()).expect("`fits` bounds the entry to an addressable size"),
    );
    (&mut entry)
        .take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut encoded)
        .map_err(|source| DocumentLoadError::ReadDocument {
            path: path.to_path_buf(),
            source,
        })?;
    if !fits(encoded.len() as u64) {
        return Err(DocumentLoadError::DocumentTooLarge {
            path: path.to_path_buf(),
            size: encoded.len() as u64,
        });
    }

    SerdeFormat::Ron.deserialize(&encoded).map_err(|source| {
        DocumentLoadError::DeserializeDocument {
            path: path.to_path_buf(),
            source: Box::new(source),
        }
    })
}

pub(crate) fn save(document: &Document, path: &Path) -> Result<(), DocumentSaveError> {
    if !has_extension(path) {
        return Err(DocumentSaveError::InvalidExtension {
            path: path.to_path_buf(),
        });
    }
    debug_assert!(
        document.validate().is_ok(),
        "a saved document is consistent: {:?}",
        document.validate()
    );

    let encoded =
        SerdeFormat::Ron
            .serialize(document)
            .map_err(|source| DocumentSaveError::Serialize {
                path: path.to_path_buf(),
                source,
            })?;
    if !fits(encoded.len() as u64) {
        return Err(DocumentSaveError::DocumentTooLarge {
            path: path.to_path_buf(),
            size: encoded.len() as u64,
        });
    }

    fs::publish(path, fs::PublicationMode::Durable, |file| {
        write_archive(file, &encoded)
    })
    .map_err(|source| DocumentSaveError::Publish {
        path: path.to_path_buf(),
        source,
    })
}

fn has_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(EXTENSION))
}

/// Whether a `size`-byte document entry is within the limit — the one
/// predicate a load and a save both hold an entry to.
const fn fits(size: u64) -> bool {
    size <= MAX_DOCUMENT_BYTES
}

fn write_archive(file: &mut File, encoded: &[u8]) -> io::Result<()> {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut archive = ZipWriter::new(file);
    archive
        .start_file(DOCUMENT_ENTRY, options)
        .map_err(io::Error::other)?;
    archive.write_all(encoded)?;
    archive.finish().map_err(io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod tests;
