//! Darkroom document archives: one validated JSON document inside a ZIP file.

use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result as AnyResult, ensure};
use common::file_utils;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::core::document::Document;

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
    #[error("{path}: {reason}", path = .path.display())]
    InvalidDocument { path: PathBuf, reason: String },
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
        .map_err(|error| DocumentLoadError::InvalidDocument {
            path: path.to_path_buf(),
            reason: format!("{error:#}"),
        })?;
    Ok(document)
}

pub(crate) fn save(document: &Document, path: &Path) -> AnyResult<()> {
    ensure_extension(path)?;
    document.validate_debug();

    let json = serde_json::to_vec_pretty(document).with_context(|| {
        format!(
            "failed to serialize {DOCUMENT_ENTRY} for {}",
            path.display()
        )
    })?;
    ensure_document_size(json.len() as u64)?;

    file_utils::publish(path, file_utils::PublicationMode::Durable, |file| {
        write_archive(file, &json)
    })
    .with_context(|| path.display().to_string())
}

pub(crate) fn with_extension(mut path: PathBuf) -> PathBuf {
    if !has_extension(&path) {
        path.set_extension(EXTENSION);
    }
    path
}

fn ensure_extension(path: &Path) -> AnyResult<()> {
    ensure!(
        has_extension(path),
        "Darkroom documents must use the .{EXTENSION} extension"
    );
    Ok(())
}

fn has_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(EXTENSION))
}

fn ensure_document_size(size: u64) -> AnyResult<()> {
    ensure!(
        size <= MAX_DOCUMENT_BYTES,
        "{DOCUMENT_ENTRY} exceeds the {} MiB size limit",
        MAX_DOCUMENT_BYTES / (1024 * 1024)
    );
    Ok(())
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
mod tests {
    use std::io::Read as _;

    use common::test_utils::test_output_path;
    use scenarium::GraphId;

    use super::*;

    #[test]
    fn document_round_trips_as_one_json_entry() {
        let path = test_output_path("darkroom_document/roundtrip.darkroom");
        let document = Document::default();

        save(&document, &path).expect("save document");
        assert_eq!(load(&path).expect("load document"), document);

        let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
        assert_eq!(archive.len(), 1, "archives contain only the document entry");
        let mut entry = archive.by_name(DOCUMENT_ENTRY).unwrap();
        assert_eq!(entry.compression(), CompressionMethod::Deflated);
        let mut json = String::new();
        entry.read_to_string(&mut json).unwrap();
        let decoded: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, document, "the archive payload is plain JSON");
    }

    #[test]
    fn document_extension_is_required_case_insensitively() {
        let document = Document::default();
        let wrong = test_output_path("darkroom_document/wrong.json");
        let error = save(&document, &wrong).unwrap_err().to_string();
        assert!(error.contains(".darkroom extension"), "{error}");
        assert!(
            matches!(
                load(&wrong).unwrap_err(),
                DocumentLoadError::InvalidExtension { path } if path == wrong
            ),
            "load reports the exact rejected path"
        );

        let uppercase = test_output_path("darkroom_document/uppercase.DARKROOM");
        save(&document, &uppercase).expect("uppercase extension is valid");
        assert_eq!(load(&uppercase).unwrap(), document);

        assert_eq!(
            with_extension(PathBuf::from("scene")),
            PathBuf::from("scene.darkroom")
        );
        assert_eq!(
            with_extension(PathBuf::from("scene.json")),
            PathBuf::from("scene.darkroom")
        );
        assert_eq!(
            with_extension(PathBuf::from("scene.DARKROOM")),
            PathBuf::from("scene.DARKROOM")
        );
    }

    #[test]
    fn load_rejects_invalid_archives_and_missing_or_invalid_documents() {
        let corrupt = test_output_path("darkroom_document/corrupt.darkroom");
        std::fs::write(&corrupt, b"not a zip archive").unwrap();
        assert!(
            matches!(
                load(&corrupt).unwrap_err(),
                DocumentLoadError::InvalidArchive { path, .. } if path == corrupt
            ),
            "corrupt ZIP reports the exact archive path"
        );

        let missing = test_output_path("darkroom_document/missing.darkroom");
        write_test_archive(&missing, "other.json", b"{}");
        assert!(
            matches!(
                load(&missing).unwrap_err(),
                DocumentLoadError::DocumentEntryCount { path, count: 0 } if path == missing
            ),
            "missing document entry reports its archive and exact count"
        );

        let malformed = test_output_path("darkroom_document/malformed.darkroom");
        write_test_archive(&malformed, DOCUMENT_ENTRY, b"{");
        assert!(
            matches!(
                load(&malformed).unwrap_err(),
                DocumentLoadError::DeserializeDocument { path, .. } if path == malformed
            ),
            "malformed JSON reports its archive"
        );

        let invalid = test_output_path("darkroom_document/invalid.darkroom");
        let mut document = Document::default();
        document.graph.origin = Some(GraphId::nil());
        let json = serde_json::to_vec(&document).unwrap();
        write_test_archive(&invalid, DOCUMENT_ENTRY, &json);
        assert!(
            matches!(
                load(&invalid).unwrap_err(),
                DocumentLoadError::InvalidDocument { path, reason }
                    if path == invalid && reason.contains("graph has a nil origin")
            ),
            "structural validation retains the archive path and reason"
        );
    }

    #[test]
    fn document_size_limit_rejects_the_first_byte_over_the_boundary() {
        ensure_document_size(MAX_DOCUMENT_BYTES).expect("boundary is accepted");
        let error = ensure_document_size(MAX_DOCUMENT_BYTES + 1)
            .unwrap_err()
            .to_string();
        assert!(error.contains("256 MiB size limit"), "{error}");

        let path = Path::new("oversized.darkroom");
        ensure_load_document_size(path, MAX_DOCUMENT_BYTES).expect("load boundary is accepted");
        assert!(matches!(
            ensure_load_document_size(path, MAX_DOCUMENT_BYTES + 1).unwrap_err(),
            DocumentLoadError::DocumentTooLarge {
                path: error_path,
                size
            } if error_path == path && size == MAX_DOCUMENT_BYTES + 1
        ));
    }

    fn write_test_archive(path: &Path, name: &str, contents: &[u8]) {
        let file = File::create(path).unwrap();
        let mut archive = ZipWriter::new(file);
        archive
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        archive.write_all(contents).unwrap();
        archive.finish().unwrap();
    }
}
