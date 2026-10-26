//! [`SavedPaths`]: how a document file holds the paths its path inputs name. In memory every path
//! is absolute; the file holds a path inside the document's folder relative to it, so the folder
//! moves or is copied whole and its files go with it. `.notes/editor-harness.md` holds the
//! decision.

use std::borrow::Cow;
use std::path::{self, Component, Path, PathBuf};

use crate::core::document::Document;
use crate::core::io::document::error::DocumentLoadError;

/// How a save writes a path that lies inside the document's folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SavedPaths {
    /// Relative to the folder, with `/` between its parts on every system.
    Relative,
    /// As the document holds it.
    Absolute,
}

impl SavedPaths {
    /// `document` as a save to `path` writes it: borrowed when no path changes.
    pub(super) fn of<'a>(self, document: &'a Document, path: &Path) -> Cow<'a, Document> {
        let folder = folder(path);
        let relative = |value: &str| relative_to(value, &folder);
        if self == Self::Absolute || !any_path(document, relative) {
            return Cow::Borrowed(document);
        }
        let mut saved = document.clone();
        saved.map_paths(relative);
        Cow::Owned(saved)
    }
}

/// Make each relative path `document` holds absolute, against the folder of `path`, the file it
/// was read from.
pub(super) fn resolve(document: &mut Document, path: &Path) -> Result<(), DocumentLoadError> {
    let folder = folder(path);
    let Some(base) = folder.to_str() else {
        if any_path(document, |value| (!absolute(value)).then_some(())) {
            return Err(DocumentLoadError::NonUnicodeFolder {
                path: path.to_path_buf(),
            });
        }
        return Ok(());
    };
    debug_assert!(!base.is_empty(), "an absolute folder");
    document.map_paths(|value| {
        if absolute(value) {
            return None;
        }
        // Part by part, so the separators are the system's own: `inputs/a.tiff` read on Windows
        // is `C:\p\inputs\a.tiff`, the path a pick there gives, not `C:\p\inputs/a.tiff`.
        let mut joined = folder.clone();
        for part in Path::new(value).components() {
            if part != Component::CurDir {
                joined.push(part);
            }
        }
        Some(
            joined
                .into_os_string()
                .into_string()
                .expect("a Unicode folder joined with a Unicode path"),
        )
    });
    Ok(())
}

/// The relative paths of `document` that climb out of the folder they are relative to, as
/// `../x.tiff` does: a copy of the folder alone would point them at other files. Sorted, each
/// once, as the order of node ids is no order a reader follows.
pub(super) fn leaving_folder(document: &Document) -> Vec<String> {
    let mut leaving = Vec::new();
    document.for_each_path(|_, value| {
        if !absolute(value) && climbs_out(value) {
            leaving.push(value.to_string());
        }
    });
    leaving.sort_unstable();
    leaving.dedup();
    leaving
}

/// The folder a document at `path` lies in, absolute: a relative path is taken against the
/// working directory, as opening it does.
fn folder(path: &Path) -> PathBuf {
    let folder = path.parent().unwrap_or(path);
    path::absolute(folder).unwrap_or_else(|_| folder.to_path_buf())
}

/// Whether `value` needs no folder: absolute, or empty, which an unset path input holds.
fn absolute(value: &str) -> bool {
    value.is_empty() || Path::new(value).is_absolute()
}

/// `value` relative to `folder` when it lies inside it; `.` for the folder itself.
fn relative_to(value: &str, folder: &Path) -> Option<String> {
    if !Path::new(value).is_absolute() {
        return None;
    }
    let inner = Path::new(value).strip_prefix(folder).ok()?;
    let parts: Vec<&str> = inner
        .components()
        .map(|part| {
            part.as_os_str()
                .to_str()
                .expect("a part of a Unicode path is Unicode")
        })
        .collect();
    Some(if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    })
}

/// Whether the relative `value`, read part by part, ever names a folder above its start.
fn climbs_out(value: &str) -> bool {
    let mut depth = 0_i32;
    for part in Path::new(value).components() {
        match part {
            Component::ParentDir => depth -= 1,
            Component::Normal(_) => depth += 1,
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
        if depth < 0 {
            return true;
        }
    }
    false
}

/// Whether `map` changes any path of `document`.
fn any_path<T>(document: &Document, mut map: impl FnMut(&str) -> Option<T>) -> bool {
    let mut any = false;
    document.for_each_path(|_, value| any |= map(value).is_some());
    any
}
