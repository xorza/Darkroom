//! [`DocumentFormat`]: the two forms a `.darkroom` file takes — the document's RON text, which a
//! person or an agent reads and git diffs line by line, or that text zipped, a fifth of its size.
//! `darkroom/AGENTS.md`, "The document file", holds the decision.

use std::borrow::Cow;
use std::fs::File;
use std::io::{self, Cursor, Read as _, Write as _};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::core::io::document::error::DocumentLoadError;
use crate::core::io::document::{MAX_DOCUMENT_BYTES, fits};

/// The one entry a zipped document holds.
pub(super) const DOCUMENT_ENTRY: &str = "document.ron";

/// How a `.darkroom` file holds its document. A load tells them apart by the file's first bytes,
/// so both share the extension and its file association.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DocumentFormat {
    /// The RON text itself.
    #[default]
    Ron,
    /// A ZIP archive of one deflated entry, [`DOCUMENT_ENTRY`], which holds the RON text.
    ZippedRon,
}

impl DocumentFormat {
    /// The format of a file whose bytes are `bytes`: a ZIP's signature, `PK`, which no RON
    /// document starts with, or RON text.
    pub(super) fn of(bytes: &[u8]) -> Self {
        if bytes.starts_with(b"PK") {
            Self::ZippedRon
        } else {
            Self::Ron
        }
    }

    /// The RON text `bytes`, the file at `path` in this format, holds.
    pub(super) fn ron<'a>(
        self,
        bytes: &'a [u8],
        path: &Path,
    ) -> Result<Cow<'a, [u8]>, DocumentLoadError> {
        match self {
            Self::Ron => Ok(Cow::Borrowed(bytes)),
            Self::ZippedRon => unzip(bytes, path).map(Cow::Owned),
        }
    }

    /// Write `ron`, a document's text, to `file` in this format.
    pub(super) fn write(self, file: &mut File, ron: &[u8]) -> io::Result<()> {
        match self {
            Self::Ron => file.write_all(ron),
            Self::ZippedRon => {
                let options =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
                let mut archive = ZipWriter::new(file);
                archive
                    .start_file(DOCUMENT_ENTRY, options)
                    .map_err(io::Error::other)?;
                archive.write_all(ron)?;
                archive.finish().map_err(io::Error::other)?;
                Ok(())
            }
        }
    }
}

/// The one [`DOCUMENT_ENTRY`] of the archive `bytes`, refusing an archive a writer of this format
/// would not make: overlapping entries, no document entry or several, or one that is no file.
fn unzip(bytes: &[u8], path: &Path) -> Result<Vec<u8>, DocumentLoadError> {
    let path = || path.to_path_buf();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|source| {
        DocumentLoadError::InvalidArchive {
            path: path(),
            source,
        }
    })?;
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
    let mut ron = Vec::with_capacity(
        usize::try_from(entry.size()).expect("`fits` bounds the entry to an addressable size"),
    );
    // One byte past the limit: the size the entry states is the archive's word, not a bound.
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
