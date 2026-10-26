//! Darkroom documents: one `.darkroom` file, the document's RON text as it is or zipped.

pub(crate) mod document_format;
pub(crate) mod error;
pub(crate) mod saved_paths;

use std::io::Read as _;
use std::path::Path;

use common::{SerdeFormat, fs};

use crate::core::document::Document;
use crate::core::io::document::document_format::DocumentFormat;
use crate::core::io::document::error::{DocumentLoadError, DocumentSaveError};
use crate::core::io::document::saved_paths::SavedPaths;

pub(crate) const EXTENSION: &str = "darkroom";
const MAX_DOCUMENT_BYTES: u64 = 256 * 1024 * 1024;

/// A document as its file held it.
#[derive(Debug)]
pub(crate) struct DocumentFile {
    pub(crate) document: Document,
    /// The form the file had, which a save of the document keeps.
    pub(crate) format: DocumentFormat,
}

/// Decode the document at `path`, in either format, each path it holds made absolute against its
/// folder. What it holds is made consistent by [`Document::reconcile`], which the caller runs
/// against its library.
pub(crate) fn load(path: &Path) -> Result<DocumentFile, DocumentLoadError> {
    let mut file = decode(path)?;
    saved_paths::resolve(&mut file.document, path)?;
    Ok(file)
}

/// The relative paths the document at `path` holds that climb out of its folder.
pub(crate) fn paths_leaving_folder(path: &Path) -> Result<Vec<String>, DocumentLoadError> {
    Ok(saved_paths::leaving_folder(&decode(path)?.document))
}

/// The document at `path` as its file holds it.
fn decode(path: &Path) -> Result<DocumentFile, DocumentLoadError> {
    if !has_extension(path) {
        return Err(DocumentLoadError::InvalidExtension {
            path: path.to_path_buf(),
        });
    }
    let file = fs::open(path).map_err(|source| DocumentLoadError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    // One byte past the limit, so a file over it is told apart from one at it without trusting
    // a size the file system reported before the read.
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| DocumentLoadError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    if !fits(bytes.len() as u64) {
        return Err(DocumentLoadError::DocumentTooLarge {
            path: path.to_path_buf(),
            size: bytes.len() as u64,
        });
    }
    let format = DocumentFormat::of(&bytes);
    let ron = format.ron(&bytes, path)?;
    let document =
        SerdeFormat::Ron
            .deserialize(&ron)
            .map_err(|source| DocumentLoadError::Deserialize {
                path: path.to_path_buf(),
                source: Box::new(source),
            })?;
    Ok(DocumentFile { document, format })
}

/// Write `document` to `path` in `format`, its paths as `paths` says.
pub(crate) fn save(
    document: &Document,
    path: &Path,
    paths: SavedPaths,
    format: DocumentFormat,
) -> Result<(), DocumentSaveError> {
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

    let encoded = SerdeFormat::Ron
        .serialize(paths.of(document, path).as_ref())
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
        format.write(file, &encoded)
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

/// Whether a `size`-byte document is within the limit — the one predicate a load and a save
/// both hold a document to.
const fn fits(size: u64) -> bool {
    size <= MAX_DOCUMENT_BYTES
}

#[cfg(test)]
pub(crate) mod internals {
    use std::path::Path;

    use common::{SerdeFormat, fs};

    use crate::core::document::Document;

    /// Write `document` to `path` as it is, unchecked: a file holding what no save writes, as a
    /// relative path that climbs out of its folder.
    pub(crate) fn write_unchecked(document: &Document, path: &Path) {
        fs::write(path, SerdeFormat::Ron.serialize(document).unwrap()).unwrap();
    }
}

#[cfg(test)]
mod tests;
