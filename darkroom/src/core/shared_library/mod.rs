//! [`SharedLibrary`]: the compounds the library folder shares, as one scan read them.
//!
//! - **A library file is an ordinary document**, and each compound it marks shared is listed; its
//!   main graph is where the author wires the compound to sample inputs. As Blender's asset
//!   library, a folder of blend-files whose data is marked an asset. Refused: a format of its
//!   own, with no place for the wiring that shows a compound working.
//! - **A library file is read as it is written**, with no library to reconcile it against off
//!   the frame: a compound with no side is shared by none and shows in a row, and what a place
//!   copies in is reconciled as it enters ([`bring_in`](crate::core::edit::bring_in)).
//! - **A scan reads the folder whole, off the frame**, once at start and when the folder changes,
//!   and keeps what it read in memory: a place copies from it and an update reads it with no
//!   disk read. A file that does not load is listed with its error; an id two files hold is one
//!   entry when their digests are equal and one per file when not.
//! - **A document keeps a copy of each library compound it uses**, and of those that one places,
//!   under the same ids, so it opens anywhere. As Blender's Append, "the asset and all its
//!   dependencies will be Appended (copied) into the current file" ([asset
//!   browser](https://docs.blender.org/manual/en/latest/editors/asset_browser.html)), and Houdini's
//!   definitions saved in the scene. Refused: a link, as Nuke's gizmo, where a script stores "only
//!   the gizmo's name and control settings": the document then needs the library beside it, and a
//!   result changes with a file it does not hold.
//! - **A shared file holds no code**: its compounds are graphs over the built-in funcs, and a
//!   func the receiver lacks comes in as a ghost node inside the compound, as in any document.
//! - **Update from Library is an undoable edit; Publish to Library asks first.** An Update keeps
//!   each instance's bindings, which name ports by id; a Publish replaces the definition inside the
//!   library file, leaves its main graph, and writes the file whole or not at all. As Simulink's
//!   Restore Link and Push Link ([disable or break
//!   links](https://www.mathworks.com/help/simulink/ug/disable-links-to-library-blocks.html)) and
//!   Houdini's Match Current Definition and Save Node Type.
//! - **A copy remembers the digest it last shared with the library** ([`SyncState`]). Refused:
//!   version numbers, which order two copies and say nothing of a third made beside them.

pub(crate) mod scanner;

use std::path::{Path, PathBuf};

use common::fs;
use scenarium::{CompoundDigest, CompoundId, Library, Manifest};

use crate::core::document::Document;
use crate::core::document::compound_side::CompoundSide;
use crate::core::document::graph_view::GraphView;
use crate::core::edit::bring_in::Carried;
use crate::core::edit::compound_intent::Update;
use crate::core::io::document;
use crate::core::io::document::document_format::DocumentFormat;
use crate::core::io::document::error::DocumentLoadError;

/// The library folder's shared compounds and the files they came from, as one scan read them.
#[derive(Debug, Default)]
pub(crate) struct SharedLibrary {
    /// Which scan read it: none for the empty library, then one more for each scan that lands,
    /// so a value cached off it knows when it is stale.
    generation: u64,
    files: Vec<LibraryFile>,
    entries: Vec<LibraryEntry>,
    failures: Vec<ScanFailure>,
}

/// One file of the folder that loaded: its path, the document it decodes to and the manifest
/// saved beside it.
#[derive(Debug)]
struct LibraryFile {
    path: PathBuf,
    document: Document,
    manifest: Manifest,
}

/// One shared compound the library lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LibraryEntry {
    pub(crate) id: CompoundId,
    pub(crate) name: String,
    pub(crate) digest: CompoundDigest,
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
/// list the compounds each marks shared. A folder that cannot be read lists nothing and names
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
            Ok(loaded) => library.add_file(path, loaded.document, loaded.manifest),
            Err(error) => library.failures.push(ScanFailure { path, error }),
        }
    }
    library
}

impl SharedLibrary {
    /// Take in `document` from `path`, listing each compound it shares that no entry lists with
    /// the same id and digest.
    fn add_file(&mut self, path: PathBuf, document: Document, manifest: Manifest) {
        let file = self.files.len();
        for (id, compound) in document.graphs.compounds() {
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
                    name: compound.name.clone(),
                    digest,
                    file,
                });
            }
        }
        self.files.push(LibraryFile {
            path,
            document,
            manifest,
        });
    }

    /// See the field.
    pub(crate) const fn generation(&self) -> u64 {
        self.generation
    }

    /// Each compound the library lists, in the order its files were read.
    pub(crate) fn entries(&self) -> &[LibraryEntry] {
        &self.entries
    }

    /// Each file of the folder that did not load.
    pub(crate) fn failures(&self) -> &[ScanFailure] {
        &self.failures
    }

    /// The entry the library lists under `id`, when it lists exactly one: the definition a copy
    /// under that id syncs with. `None` when it lists none, or two that differ.
    pub(crate) fn entry(&self, id: CompoundId) -> Option<&LibraryEntry> {
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

    /// The manifest of the file that holds `entry`: what its funcs declared when it was saved,
    /// which a place and an update read its definitions against.
    pub(crate) fn manifest(&self, entry: &LibraryEntry) -> &Manifest {
        &self.files[entry.file].manifest
    }

    /// `entry`'s definition and each one it places, however deep, each after those it places,
    /// with its view: what placing it copies in. Each remembers its digest as the one it last
    /// shared with the library.
    /// The update of each compound of `document` that follows the library and whose library copy
    /// moved while it stayed as it last synced, in compound-id order: what a scan brings a
    /// follower.
    pub(crate) fn follower_updates(&self, document: &Document) -> Vec<Update> {
        document
            .sides
            .iter()
            .filter(|(_, side)| side.follows)
            .filter_map(|(&id, side)| {
                let entry = self.entry(id)?;
                let state = SyncState::of(side.synced, document.graphs.digest(id), entry.digest);
                (state == SyncState::UpdateAvailable).then(|| Update {
                    id,
                    carried: self.carried(entry),
                    manifest: self.manifest(entry).clone(),
                })
            })
            .collect()
    }

    pub(crate) fn carried(&self, entry: &LibraryEntry) -> Vec<Carried> {
        let document = self.document(entry);
        document
            .graphs
            .placed_first(entry.id)
            .into_iter()
            .map(|id| {
                let compound = document
                    .graphs
                    .compound(id)
                    .expect("a placed compound is in its file")
                    .clone();
                let side = document.sides.get(&id);
                let view =
                    side.map_or_else(|| GraphView::new(&compound.graph), |side| side.view.clone());
                Carried {
                    id,
                    compound,
                    view,
                    synced: Some(document.graphs.digest(id)),
                    locked: side.is_some_and(|side| side.locked),
                }
            })
            .collect()
    }
}

/// `file`, a library document, with `document`'s compound `id` and each one it places written in
/// place of the definitions it holds under their ids, or beside them, each marked as last shared
/// at its digest and `id` marked shared, then reconciled against `library` and `manifest`, the
/// file's own, as what places them there may no longer fit their ports. What a publish writes
/// whole over the file.
pub(crate) fn published(
    file: &Document,
    manifest: &Manifest,
    document: &Document,
    id: CompoundId,
    library: &Library,
) -> Document {
    let mut file = file.clone();
    for placed in document.graphs.placed_first(id) {
        let definition = document
            .graphs
            .compound(placed)
            .expect("a placed compound is held")
            .clone();
        let side = CompoundSide {
            through: None,
            synced: Some(document.graphs.digest(placed)),
            shared: placed == id || file.sides.get(&placed).is_some_and(|held| held.shared),
            ..document.sides[&placed].clone()
        };
        file.graphs.put_compound(placed, definition);
        file.sides.insert(placed, side);
    }
    for repair in file.reconcile(library, manifest).repairs {
        tracing::info!(target: "darkroom::library", "publishing: {repair}");
    }
    file
}

/// Where a copy of a library compound stands against the library, read off three digests: the
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
        synced: Option<CompoundDigest>,
        local: CompoundDigest,
        library: CompoundDigest,
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
