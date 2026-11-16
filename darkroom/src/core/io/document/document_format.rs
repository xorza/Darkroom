//! [`DocumentFormat`]: the two forms a project file takes — the document's RON text in a
//! `.darkroom` file, which a person or an agent reads and git diffs line by line, or that text
//! zipped in a `.darkroomz` file, a fifth of its size. `darkroom/AGENTS.md`, "The document file",
//! holds the decision.

use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::path::Path;

use common::fs;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::core::io::document::error::{DocumentLoadError, DocumentWriteError};
use crate::core::io::document::resources::Bundled;
use crate::core::io::document::{MAX_DOCUMENT_BYTES, fits};

/// The one entry a zipped document holds.
pub(super) const DOCUMENT_ENTRY: &str = "document.ron";

/// The extension of a project file of RON text.
pub(crate) const RON_EXTENSION: &str = "darkroom";

/// The extension of a zipped project file.
pub(crate) const ZIPPED_EXTENSION: &str = "darkroomz";

/// How a project file holds its document. Its extension names the form a save writes, as `.svg`
/// and `.svgz` do; a load reads the form its first bytes show, so a misnamed file still opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DocumentFormat {
    /// The RON text itself.
    Ron,
    /// A ZIP archive of deflated entries: [`DOCUMENT_ENTRY`], which holds the RON text, and the
    /// files it bundles under `resources/`.
    ZippedRon,
}

impl DocumentFormat {
    pub(crate) const ALL: [Self; 2] = [Self::Ron, Self::ZippedRon];

    /// The extension a file of this format takes.
    pub(crate) const fn extension(self) -> &'static str {
        match self {
            Self::Ron => RON_EXTENSION,
            Self::ZippedRon => ZIPPED_EXTENSION,
        }
    }

    /// The format a file named `path` is saved in: the one its extension names, without regard to
    /// case, or none for another extension.
    pub(crate) fn of_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        Self::ALL
            .into_iter()
            .find(|format| extension.eq_ignore_ascii_case(format.extension()))
    }

    /// The format of a file whose bytes are `bytes`: a ZIP's signature, `PK`, which no RON
    /// document starts with, or RON text.
    pub(super) fn of(bytes: &[u8]) -> Self {
        if bytes.starts_with(b"PK") {
            Self::ZippedRon
        } else {
            Self::Ron
        }
    }

    /// Write `ron`, a document's text, to `file` in this format, with each of `resources` beside
    /// it in an archive, which a plain text file holds none of.
    pub(super) fn write(
        self,
        file: &mut File,
        ron: &[u8],
        resources: &[Bundled],
    ) -> Result<(), DocumentWriteError> {
        match self {
            Self::Ron => {
                debug_assert!(resources.is_empty(), "plain text bundles no file");
                Ok(file.write_all(ron)?)
            }
            Self::ZippedRon => {
                let options =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
                let mut archive = ZipWriter::new(file);
                archive.start_file(DOCUMENT_ENTRY, options)?;
                archive.write_all(ron)?;
                for resource in resources {
                    let resource_error = |source| DocumentWriteError::Resource {
                        path: resource.source.clone(),
                        source,
                    };
                    let mut source = fs::open(&resource.source).map_err(resource_error)?;
                    archive.start_file(resource.name.as_str(), options)?;
                    io::copy(&mut source, &mut archive).map_err(resource_error)?;
                }
                archive.finish()?;
                Ok(())
            }
        }
    }
}

/// The archive `file`, the file at `path`, read where it lies.
pub(super) fn open_archive(file: File, path: &Path) -> Result<ZipArchive<File>, DocumentLoadError> {
    ZipArchive::new(file).map_err(|source| DocumentLoadError::InvalidArchive {
        path: path.to_path_buf(),
        source,
    })
}

/// The one [`DOCUMENT_ENTRY`] of `archive`, the file at `path`, refusing an archive a writer of
/// this format would not make: overlapping entries, no document entry or several, or one that is
/// no file.
pub(super) fn unzip(
    archive: &mut ZipArchive<File>,
    path: &Path,
) -> Result<Vec<u8>, DocumentLoadError> {
    let path = || path.to_path_buf();
    let overlapping =
        archive
            .has_overlapping_files()
            .map_err(|source| DocumentLoadError::InspectArchive {
                path: path(),
                source,
            })?;
    if overlapping {
        return Err(DocumentLoadError::OverlappingEntries { path: path() });
    }
    let count = archive
        .file_names()
        .try_fold(0, |count, name| {
            name.map(|name| count + usize::from(name == DOCUMENT_ENTRY))
        })
        .map_err(|source| DocumentLoadError::InspectArchive {
            path: path(),
            source,
        })?;
    if count != 1 {
        return Err(DocumentLoadError::DocumentEntryCount {
            path: path(),
            count,
        });
    }
    let mut entry =
        archive
            .by_name(DOCUMENT_ENTRY)
            .map_err(|source| DocumentLoadError::OpenDocumentEntry {
                path: path(),
                source,
            })?;
    if !entry.is_file() {
        return Err(DocumentLoadError::NonFileDocumentEntry { path: path() });
    }
    if !fits(entry.size()) {
        return Err(DocumentLoadError::DocumentTooLarge {
            path: path(),
            size: entry.size(),
        });
    }
    // Grown as it reads, and one byte past the limit: the size the entry states is the archive's
    // word, which a reserve would trust with up to the whole limit for a file of a few bytes.
    let mut ron = Vec::new();
    (&mut entry)
        .take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut ron)
        .map_err(|source| DocumentLoadError::Read {
            path: path(),
            source,
        })?;
    if !fits(ron.len() as u64) {
        return Err(DocumentLoadError::DocumentTooLarge {
            path: path(),
            size: ron.len() as u64,
        });
    }
    Ok(ron)
}
