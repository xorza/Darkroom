//! The files a `.darkroomz` carries beside its document, under `resources/`, and their unpack
//! beside the archive.
//!
//! - **An entry is named by the digest of its content**, `resources/<digest>/<file name>`, so two
//!   files of one name do not clash, one file bundled twice is one entry, and a changed file is a
//!   new entry. Refused: the file name alone with a size and modified-time check, which is a guess
//!   about what a name holds.
//! - **Resources unpack beside the archive**, into `<archive file name>-resources/`, as the disk
//!   cache sits in `-cache/`, so a path in memory is an absolute path as any other and no node,
//!   codec or digest changes. Refused: reading the archive in place, which every path reader would
//!   have to know; a temp folder per session, which unpacks at every open. Cost: the bytes are on
//!   disk twice.
//! - **An unpacked file is whole, checked and read-only**: written under a temporary name, held to
//!   the digest its entry's name gives and renamed into place, so a file that is there is the one
//!   the archive holds. As Go keeps its module cache read-only.
//! - **A save of an archive bundles each unpacked file its document names**, of any archive's
//!   resources folder, under the digest its folder gives, so a Save As keeps them; an export adds
//!   the files the person picked, hashed as they are read. A bundled file streams from disk into
//!   the archive, and an archive is read where it lies, so neither holds every file in memory.
//! - **Only a regular file under `resources/` is unpacked**: an entry whose name the zip crate's
//!   `enclosed_name` refuses, a link, a name not of the digest form, or an entry longer or shorter
//!   than its header says is refused and named in the load report, as an archive is another
//!   person's data ([CVE-2025-29787](https://www.sentinelone.com/vulnerability-database/cve-2025-29787/)).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read as _};
use std::path::{Component, Path, PathBuf};

use common::fs;
use thiserror::Error;
use zip::ZipArchive;
use zip::result::ZipError;

use crate::core::document::Document;
use crate::core::io::document::error::DocumentSaveError;

/// The folder of an archive's entries, and the prefix a bundled path reads in its document.
pub(crate) const RESOURCES: &str = "resources";

/// The most bytes one resource entry may hold: past it the entry is refused, so a crafted archive
/// cannot fill a disk.
pub(crate) const MAX_RESOURCE_BYTES: u64 = 4 << 30;

/// The folder an archive's resources unpack into: `<archive file name>-resources` beside it.
pub(crate) fn folder(archive: &Path) -> PathBuf {
    let mut name = archive
        .file_name()
        .map_or_else(Default::default, ToOwned::to_owned);
    name.push("-resources");
    archive.with_file_name(name)
}

/// One file a save bundles: its entry name and the file it copies.
#[derive(Debug)]
pub(crate) struct Bundled {
    pub(crate) name: String,
    pub(crate) source: PathBuf,
}

/// Why an entry a document names was not unpacked: named in the load report, and the paths into it
/// stay unresolved.
#[derive(Debug, Error)]
pub(crate) enum RefusedEntry {
    #[error("{name} is no resource entry of the form resources/<digest>/<file name>")]
    Unnamed { name: String },
    #[error("the archive holds no entry {name}")]
    Missing { name: String },
    #[error("{name} is not a regular file")]
    NotAFile { name: String },
    #[error("{name} holds {size} bytes, over the {max} byte limit", max = MAX_RESOURCE_BYTES)]
    TooLarge { name: String, size: u64 },
    #[error("{name} is not as long as its header says")]
    WrongLength { name: String },
    #[error("{name} does not hold the content its name's digest names")]
    DigestMismatch { name: String },
    #[error("could not read {name}")]
    Read {
        name: String,
        #[source]
        source: ZipError,
    },
    #[error("could not unpack into {path}", path = .path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Unpack each entry of `zip`, the archive at `archive`, that `document` names by a
/// `resources/…` path into the archive's resources folder, putting the unpacked file's absolute
/// path in place of each; remove each digest folder there no entry names. An entry already
/// unpacked is not written again. What it refused, each once.
pub(crate) fn unpack(
    zip: &mut ZipArchive<File>,
    archive: &Path,
    document: &mut Document,
) -> Vec<RefusedEntry> {
    let prefix = format!("{RESOURCES}/");
    let mut named = BTreeSet::new();
    document.for_each_path(|_, value| {
        if value.starts_with(&prefix) {
            named.insert(value.to_string());
        }
    });
    let mut refused = Vec::new();
    let root = folder(archive);
    let mut unpacked: BTreeMap<String, String> = BTreeMap::new();
    let mut digests: BTreeSet<String> = BTreeSet::new();
    for name in named {
        match unpack_entry(zip, &root, &name) {
            Ok(path) => {
                let digest = digest_of(&name).expect("an unpacked name is of the form");
                digests.insert(digest.to_string());
                unpacked.insert(name, path);
            }
            Err(refusal) => refused.push(refusal),
        }
    }
    document.map_paths(|value| unpacked.get(value).cloned());
    remove_unnamed(&root, &digests);
    refused
}

/// The digest part of `name`, a `resources/<digest>/<file name>` entry name, when it has that form.
fn digest_of(name: &str) -> Option<&str> {
    let mut parts = name.split('/');
    let (prefix, digest, file) = (parts.next()?, parts.next()?, parts.next()?);
    let well_formed =
        prefix == RESOURCES && parts.next().is_none() && is_digest(digest) && is_file_name(file);
    well_formed.then_some(digest)
}

/// Whether `name` reads as a digest an entry's name gives: 64 lowercase hex digits, as blake3
/// writes one.
fn is_digest(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Whether `name` is one plain file name, no folder, root or `..` in it.
fn is_file_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

/// The entry an unpacked file at `path`, `<archive>-resources/<digest>/<file name>`, came from,
/// when `path` has that form.
fn unpacked_entry(path: &Path) -> Option<String> {
    let file = path.file_name()?.to_str()?;
    let digest_folder = path.parent()?;
    let digest = digest_folder.file_name()?.to_str()?;
    let root = digest_folder.parent()?.file_name()?.to_str()?;
    let unpacked = root.ends_with("-resources") && is_digest(digest) && is_file_name(file);
    unpacked.then(|| format!("{RESOURCES}/{digest}/{file}"))
}

/// Unpack the entry `name` of `zip` into `root`, or find it there unpacked; its absolute path.
fn unpack_entry(
    zip: &mut ZipArchive<File>,
    root: &Path,
    name: &str,
) -> Result<String, RefusedEntry> {
    let unnamed = || RefusedEntry::Unnamed {
        name: name.to_string(),
    };
    let digest = digest_of(name).ok_or_else(unnamed)?;
    let file = name.rsplit('/').next().ok_or_else(unnamed)?;
    let target = root.join(digest).join(file);
    let path = target.to_str().ok_or_else(unnamed)?.to_string();
    if fs::is_file(&target) {
        return Ok(path);
    }
    let mut entry = zip.by_name(name).map_err(|source| match source {
        ZipError::FileNotFound => RefusedEntry::Missing {
            name: name.to_string(),
        },
        source => RefusedEntry::Read {
            name: name.to_string(),
            source,
        },
    })?;
    if entry
        .enclosed_name()
        .is_none_or(|enclosed| enclosed != Path::new(name))
    {
        return Err(unnamed());
    }
    if !entry.is_file() || entry.is_symlink() {
        return Err(RefusedEntry::NotAFile {
            name: name.to_string(),
        });
    }
    let size = entry.size();
    if size > MAX_RESOURCE_BYTES {
        return Err(RefusedEntry::TooLarge {
            name: name.to_string(),
            size,
        });
    }
    // One byte past the header's size, so an entry longer than it says is told apart.
    let mut bytes = Vec::new();
    (&mut entry)
        .take(size + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| RefusedEntry::Read {
            name: name.to_string(),
            source: ZipError::Io(source),
        })?;
    if bytes.len() as u64 != size {
        return Err(RefusedEntry::WrongLength {
            name: name.to_string(),
        });
    }
    if blake3::hash(&bytes).to_hex().as_str() != digest {
        return Err(RefusedEntry::DigestMismatch {
            name: name.to_string(),
        });
    }
    let write = |source| RefusedEntry::Write {
        path: target.clone(),
        source,
    };
    fs::create_dir_all(root.join(digest)).map_err(write)?;
    fs::publish_bytes(&target, &bytes, fs::PublicationMode::Durable).map_err(write)?;
    let mut permissions = fs::metadata(&target).map_err(write)?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&target, permissions).map_err(write)?;
    Ok(path)
}

/// Remove each folder of `root` named as a digest that `named` does not hold: what an earlier
/// open unpacked and the document no longer names. Anything else there stays.
fn remove_unnamed(root: &Path, named: &BTreeSet<String>) {
    let Ok(read) = fs::read_dir(root) else {
        return;
    };
    for entry in read.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if is_digest(name) && !named.contains(name) {
            // A folder that does not go stays, as nothing reads it.
            fs::remove_dir_all(entry.path()).ok();
        }
    }
}

/// What a save of an archive writes: the document with each bundled path its entry's name, and
/// each entry once.
#[derive(Debug)]
pub(crate) struct Bundle {
    pub(crate) document: Document,
    pub(crate) files: Vec<Bundled>,
}

/// The files a save of `document` bundles: each unpacked file it names, under the entry it came
/// from, and each path in `extra`, hashed off the disk. A file of `extra` that cannot be read is
/// the error, with its path.
pub(crate) fn bundle(
    document: &Document,
    extra: &BTreeSet<String>,
) -> Result<Bundle, DocumentSaveError> {
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    document.for_each_path(|_, value| {
        if let Some(entry) = unpacked_entry(Path::new(value)) {
            names.insert(value.to_string(), entry);
        }
    });
    for path in extra {
        if names.contains_key(path) {
            continue;
        }
        let unreadable = |source| DocumentSaveError::Resource {
            path: PathBuf::from(path),
            source,
        };
        let file = Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| is_file_name(name))
            .unwrap_or("resource");
        let mut hasher = blake3::Hasher::new();
        hasher
            .update_reader(fs::open(path).map_err(unreadable)?)
            .map_err(unreadable)?;
        names.insert(
            path.clone(),
            format!("{RESOURCES}/{}/{file}", hasher.finalize().to_hex()),
        );
    }
    // One entry a name: two paths to one content bundle it once.
    let mut sources: BTreeMap<&str, &str> = BTreeMap::new();
    for (path, name) in &names {
        sources.entry(name).or_insert(path);
    }
    let files = sources
        .into_iter()
        .map(|(name, path)| Bundled {
            name: name.to_string(),
            source: PathBuf::from(path),
        })
        .collect();
    let mut saved = document.clone();
    saved.map_paths(|value| names.get(value).cloned());
    Ok(Bundle {
        document: saved,
        files,
    })
}

#[cfg(test)]
mod tests;
