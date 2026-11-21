//! Darkroom documents: one project file, the document's RON text as it is in a `.darkroom` file or
//! zipped in a `.darkroomz` one.
//!
//! - **The file holds the document and its manifest**, the saved declaration of each func its
//!   graphs name ([`scenarium::Manifest`]). A save derives the manifest from the library and a
//!   load hands it to [`Document::reconcile`]; memory keeps none, as the file holds paths
//!   relative and memory absolute ([`SavedPaths`]).

pub(crate) mod document_format;
pub(crate) mod error;
pub(crate) mod resources;
pub(crate) mod saved_paths;

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read as _, Seek as _};
use std::path::Path;

use common::{SerdeFormat, fs};
use scenarium::{Library, Manifest};
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::core::document::Document;
use crate::core::io::document::document_format::DocumentFormat;
use crate::core::io::document::error::{DocumentLoadError, DocumentSaveError, DocumentWriteError};
use crate::core::io::document::resources::{Bundle, RefusedEntry};
use crate::core::io::document::saved_paths::SavedPaths;

const MAX_DOCUMENT_BYTES: u64 = 256 * 1024 * 1024;

/// A document read from its file with the manifest saved beside it, and each resource of its
/// archive it names that was not unpacked.
#[derive(Debug)]
pub(crate) struct Loaded {
    pub(crate) document: Document,
    pub(crate) manifest: Manifest,
    pub(crate) refused: Vec<RefusedEntry>,
}

/// What a file holds, as a save writes it.
#[derive(Clone, Copy, Debug, Serialize)]
struct FileRef<'a> {
    document: &'a Document,
    manifest: &'a Manifest,
}

/// What a file holds, as a load reads it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileOwned {
    document: Document,
    manifest: Manifest,
}

/// Decode the document at `path`, in either format, unpack the resources of an archive it names
/// beside it, and make each path it holds absolute against its folder. What it holds is made
/// consistent by [`Document::reconcile`], which the caller runs against its library.
pub(crate) fn load(path: &Path) -> Result<Loaded, DocumentLoadError> {
    let Decoded {
        file: FileOwned {
            mut document,
            manifest,
        },
        archive,
    } = decode(path)?;
    let refused = match archive {
        Some(mut archive) => resources::unpack(&mut archive, path, &mut document),
        None => Vec::new(),
    };
    saved_paths::resolve(&mut document, path)?;
    Ok(Loaded {
        document,
        manifest,
        refused,
    })
}

/// What a file holds, and the archive that held it, which a plain text file is not.
#[derive(Debug)]
struct Decoded {
    file: FileOwned,
    archive: Option<ZipArchive<File>>,
}

/// The document at `path` as its file holds it, in the format its bytes show.
fn decode(path: &Path) -> Result<Decoded, DocumentLoadError> {
    if DocumentFormat::of_path(path).is_none() {
        return Err(DocumentLoadError::InvalidExtension {
            path: path.to_path_buf(),
        });
    }
    let read = |source| DocumentLoadError::Read {
        path: path.to_path_buf(),
        source,
    };
    let mut file = fs::open(path).map_err(|source| DocumentLoadError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let mut signature = Vec::new();
    (&mut file)
        .take(2)
        .read_to_end(&mut signature)
        .map_err(read)?;
    file.rewind().map_err(read)?;
    // An archive is read where it lies, as the files it bundles may hold far more than the
    // document limit; plain text is read whole.
    let (ron, archive) = match DocumentFormat::of(&signature) {
        DocumentFormat::Ron => {
            // One byte past the limit, so a file over it is told apart from one at it without
            // trusting a size the file system reported before the read.
            let mut bytes = Vec::new();
            file.take(MAX_DOCUMENT_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(read)?;
            if !fits(bytes.len() as u64) {
                return Err(DocumentLoadError::DocumentTooLarge {
                    path: path.to_path_buf(),
                    size: bytes.len() as u64,
                });
            }
            (bytes, None)
        }
        DocumentFormat::ZippedRon => {
            let mut archive = document_format::open_archive(file, path)?;
            (document_format::unzip(&mut archive, path)?, Some(archive))
        }
    };
    let file =
        SerdeFormat::Ron
            .deserialize(&ron)
            .map_err(|source| DocumentLoadError::Deserialize {
                path: path.to_path_buf(),
                source: Box::new(source),
            })?;
    Ok(Decoded { file, archive })
}

/// Write `document` to `path` in the format its extension names, with the manifest `library`
/// gives its funcs and its paths as `paths` says: an archive bundles each file its resources
/// folder holds that the document names.
pub(crate) fn save(
    document: &Document,
    library: &Library,
    path: &Path,
    paths: SavedPaths,
) -> Result<(), DocumentSaveError> {
    let manifest = document.graphs.manifest(library);
    write(
        FileRef {
            document,
            manifest: &manifest,
        },
        path,
        paths,
        &BTreeSet::new(),
    )
}

/// [`save`] with its paths relative, bundling each file of `extra` too: what an export picked
/// to carry. Only an archive bundles.
pub(crate) fn save_bundling(
    document: &Document,
    library: &Library,
    path: &Path,
    extra: &BTreeSet<String>,
) -> Result<(), DocumentSaveError> {
    let manifest = document.graphs.manifest(library);
    write(
        FileRef {
            document,
            manifest: &manifest,
        },
        path,
        SavedPaths::Relative,
        extra,
    )
}

fn write(
    file: FileRef<'_>,
    path: &Path,
    paths: SavedPaths,
    extra: &BTreeSet<String>,
) -> Result<(), DocumentSaveError> {
    let FileRef { document, manifest } = file;
    let Some(format) = DocumentFormat::of_path(path) else {
        return Err(DocumentSaveError::InvalidExtension {
            path: path.to_path_buf(),
        });
    };
    debug_assert!(
        document.validate().is_ok(),
        "a saved document is consistent: {:?}",
        document.validate()
    );

    let Bundle {
        document: bundled,
        files,
    } = match format {
        DocumentFormat::ZippedRon => resources::bundle(document, extra)?,
        DocumentFormat::Ron if extra.is_empty() => Bundle {
            document: document.clone(),
            files: Vec::new(),
        },
        DocumentFormat::Ron => {
            return Err(DocumentSaveError::BundleInPlainText {
                path: path.to_path_buf(),
            });
        }
    };
    let encoded = SerdeFormat::Ron
        .serialize(&FileRef {
            document: paths.of(&bundled, path).as_ref(),
            manifest,
        })
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
        format.write(file, &encoded, &files)
    })
    .map_err(|error| {
        let path = path.to_path_buf();
        match error {
            DocumentWriteError::Io(source) => DocumentSaveError::Publish { path, source },
            DocumentWriteError::Archive(source) => DocumentSaveError::Archive { path, source },
            DocumentWriteError::Resource { path, source } => {
                DocumentSaveError::Resource { path, source }
            }
        }
    })
}

/// Whether a `size`-byte document is within the limit — the one predicate a load and a save
/// both hold a document to.
const fn fits(size: u64) -> bool {
    size <= MAX_DOCUMENT_BYTES
}

/// The relative paths the document at `path` holds that climb out of its folder.
pub(crate) fn paths_leaving_folder(path: &Path) -> Result<Vec<String>, DocumentLoadError> {
    Ok(saved_paths::leaving_folder(&decode(path)?.file.document))
}

#[cfg(test)]
pub(crate) mod internals {
    use std::path::Path;

    use common::{SerdeFormat, fs};
    use scenarium::Manifest;

    use crate::core::document::Document;
    use crate::core::io::document::FileRef;

    /// Write `document` to `path` as it is, unchecked, with an empty manifest: a file holding
    /// what no save writes, as a relative path that climbs out of its folder.
    pub(crate) fn write_unchecked(document: &Document, path: &Path) {
        let file = FileRef {
            document,
            manifest: &Manifest::default(),
        };
        fs::write(path, SerdeFormat::Ron.serialize(&file).unwrap()).unwrap();
    }
}

#[cfg(test)]
mod tests;
