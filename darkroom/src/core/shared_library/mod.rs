//! [`SharedLibrary`]: the subgraphs the library folder shares, as one scan read them.
//!
//! - **A library file is an ordinary document**, and each subgraph it marks shared is listed; its
//!   main graph is where the author wires the subgraph to sample inputs. As Blender's asset
//!   library, a folder of blend-files whose data is marked an asset. Refused: a format of its
//!   own, with no place for the wiring that shows a subgraph working.
//! - **A library file is read as it is written**, with no library to reconcile it against off
//!   the frame: a subgraph with no side is shared by none and shows in a row, and what a place
//!   copies in is reconciled as it enters ([`bring_in`](crate::core::edit::bring_in)).
//! - **A scan reads the folder whole, off the frame**, once at start and when the folder changes,
//!   and keeps what it read in memory: a place copies from it and an update reads it with no
//!   disk read. A file that does not load is listed with its error; an id two files hold is one
//!   entry when their digests are equal and one per file when not.

pub(crate) mod scanner;

use std::path::{Path, PathBuf};

use common::fs;
use scenarium::{Library, SubgraphDigest, SubgraphId};

use crate::core::document::Document;
use crate::core::document::graph_view::GraphView;
use crate::core::document::subgraph_side::SubgraphSide;
use crate::core::edit::bring_in::Carried;
use crate::core::io::document;
use crate::core::io::document::document_format::DocumentFormat;
use crate::core::io::document::error::DocumentLoadError;

/// The library folder's shared subgraphs and the files they came from, as one scan read them.
#[derive(Debug, Default)]
pub(crate) struct SharedLibrary {
    /// Which scan read it: none for the empty library, then one more for each scan that lands,
    /// so a value cached off it knows when it is stale.
    generation: u64,
    files: Vec<LibraryFile>,
    entries: Vec<LibraryEntry>,
    failures: Vec<ScanFailure>,
}

/// One file of the folder that loaded: its path and the document it decodes to.
#[derive(Debug)]
struct LibraryFile {
    path: PathBuf,
    document: Document,
}

/// One shared subgraph the library lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LibraryEntry {
    pub(crate) id: SubgraphId,
    pub(crate) name: String,
    pub(crate) digest: SubgraphDigest,
    /// Which of the scan's files holds it.
    file: usize,
}

/// A file of the folder that did not load, and why.
#[derive(Debug)]
pub(crate) struct ScanFailure {
    pub(crate) path: PathBuf,
    pub(crate) error: DocumentLoadError,
}

/// Read every document of `folder`, each `.darkroom` and `.darkroomz` file in name order, and
/// list the subgraphs each marks shared. A folder that cannot be read lists nothing and names
/// itself as the failure.
pub(crate) fn scan(folder: &Path) -> SharedLibrary {
    let mut library = SharedLibrary::default();
    let mut paths: Vec<PathBuf> = match fs::read_dir(folder) {
        Ok(read) => read
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| DocumentFormat::of_path(path).is_some())
            .collect(),
        Err(source) => {
            library.failures.push(ScanFailure {
                path: folder.to_path_buf(),
                error: DocumentLoadError::Open {
                    path: folder.to_path_buf(),
                    source,
                },
            });
            return library;
        }
    };
    paths.sort_unstable();
    for path in paths {
        match document::load(&path) {
            Ok(document) => library.add_file(path, document),
            Err(error) => library.failures.push(ScanFailure { path, error }),
        }
    }
    library
}

impl SharedLibrary {
    /// Take in `document` from `path`, listing each subgraph it shares that no entry lists with
    /// the same id and digest.
    fn add_file(&mut self, path: PathBuf, document: Document) {
        let file = self.files.len();
        for (id, subgraph) in document.graphs.subgraphs() {
            if !document.sides.get(&id).is_some_and(|side| side.shared) {
                continue;
            }
            let digest = document.graphs.digest(id);
            let listed = self
                .entries
                .iter()
                .any(|entry| entry.id == id && entry.digest == digest);
            if !listed {
                self.entries.push(LibraryEntry {
                    id,
                    name: subgraph.name.clone(),
                    digest,
                    file,
                });
            }
        }
        self.files.push(LibraryFile { path, document });
    }

    /// See the field.
    pub(crate) const fn generation(&self) -> u64 {
        self.generation
    }

    /// Each subgraph the library lists, in the order its files were read.
    pub(crate) fn entries(&self) -> &[LibraryEntry] {
        &self.entries
    }

    /// Each file of the folder that did not load.
    pub(crate) fn failures(&self) -> &[ScanFailure] {
        &self.failures
    }

    /// The entry the library lists under `id`, when it lists exactly one: the definition a copy
    /// under that id syncs with. `None` when it lists none, or two that differ.
    pub(crate) fn entry(&self, id: SubgraphId) -> Option<&LibraryEntry> {
        let mut listed = self.entries.iter().filter(|entry| entry.id == id);
        let entry = listed.next()?;
        listed.next().is_none().then_some(entry)
    }

    /// The file that holds `entry`.
    pub(crate) fn path(&self, entry: &LibraryEntry) -> &Path {
        &self.files[entry.file].path
    }

    /// The document of the file that holds `entry`, as the scan read it.
    pub(crate) fn document(&self, entry: &LibraryEntry) -> &Document {
        &self.files[entry.file].document
    }

    /// `entry`'s definition and each one it places, however deep, each after those it places,
    /// with its view: what placing it copies in. Each remembers its digest as the one it last
    /// shared with the library.
    pub(crate) fn carried(&self, entry: &LibraryEntry) -> Vec<Carried> {
        let document = self.document(entry);
        document
            .graphs
            .placed_first(entry.id)
            .into_iter()
            .map(|id| {
                let subgraph = document
                    .graphs
                    .subgraph(id)
                    .expect("a placed subgraph is in its file")
                    .clone();
                let view = document
                    .sides
                    .get(&id)
                    .map_or_else(|| GraphView::new(&subgraph.graph), |side| side.view.clone());
                Carried {
                    id,
                    subgraph,
                    view,
                    synced: Some(document.graphs.digest(id)),
                }
            })
            .collect()
    }
}

/// `file`, a library document, with `document`'s subgraph `id` and each one it places written in
/// place of the definitions it holds under their ids, or beside them, each marked as last shared
/// at its digest and `id` marked shared, then reconciled against `library`, as what places them
/// there may no longer fit their ports. What a publish writes whole over the file.
pub(crate) fn published(
    file: &Document,
    document: &Document,
    id: SubgraphId,
    library: &Library,
) -> Document {
    let mut file = file.clone();
    for placed in document.graphs.placed_first(id) {
        let definition = document
            .graphs
            .subgraph(placed)
            .expect("a placed subgraph is held")
            .clone();
        let side = SubgraphSide {
            through: None,
            synced: Some(document.graphs.digest(placed)),
            shared: placed == id || file.sides.get(&placed).is_some_and(|held| held.shared),
            ..document.sides[&placed].clone()
        };
        file.graphs.put_subgraph(placed, definition);
        file.sides.insert(placed, side);
    }
    for repair in file.reconcile(library).repairs {
        tracing::info!(target: "darkroom::library", "publishing: {repair}");
    }
    file
}

/// Where a copy of a library subgraph stands against the library, read off three digests: the
/// one it last shared, its own and the library's. A two-way comparison cannot tell changed in
/// both from either other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SyncState {
    InSync,
    /// The library changed since the copy last shared with it; the copy did not.
    UpdateAvailable,
    /// The copy changed since; the library did not.
    ChangedHere,
    /// Both changed since.
    ChangedInBoth,
}

impl SyncState {
    /// Where a copy whose own digest is `local`, which last shared `synced` with a library
    /// holding `library`, stands: in sync when the two definitions are equal, whatever the last
    /// share. A copy that never shared, `synced` none, and differs reads changed in both.
    pub(crate) fn of(
        synced: Option<SubgraphDigest>,
        local: SubgraphDigest,
        library: SubgraphDigest,
    ) -> Self {
        if local == library {
            return Self::InSync;
        }
        match (synced == Some(local), synced == Some(library)) {
            (true, _) => Self::UpdateAvailable,
            (false, true) => Self::ChangedHere,
            (false, false) => Self::ChangedInBoth,
        }
    }

    /// The words the pane header shows it as.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::InSync => "in sync with the library",
            Self::UpdateAvailable => "update available",
            Self::ChangedHere => "changed here",
            Self::ChangedInBoth => "changed here and in the library",
        }
    }
}

#[cfg(test)]
mod tests;
