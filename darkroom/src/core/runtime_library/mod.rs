//! The ephemeral runtime registry shared by every frontend.

use std::sync::{Arc, RwLock};

use lens::{MlModelPaths, astro_library, fs_watch_library, image_library, random_library};
use scenarium::Library as ScenariumLibrary;
use scenarium::{GraphDef, NodeId, math_library, system_library, worker_events_library};

use crate::core::document::{Document, GraphRef};
use crate::core::edit::publish;
use crate::core::graph_library::GraphLibrary;
use crate::core::io::graph_library as graph_library_io;
use crate::core::io::graph_library::{GraphLibraryLoadError, GraphLibrarySaveError};

#[derive(Clone, Debug)]
pub(crate) struct PublishedLibrary {
    current: Arc<RwLock<Arc<ScenariumLibrary>>>,
}

impl PublishedLibrary {
    fn new(current: Arc<ScenariumLibrary>) -> Self {
        Self {
            current: Arc::new(RwLock::new(current)),
        }
    }

    pub(crate) fn load(&self) -> Arc<ScenariumLibrary> {
        self.current.read().unwrap().clone()
    }

    fn replace(&self, current: Arc<ScenariumLibrary>) {
        *self.current.write().unwrap() = current;
    }
}

#[derive(Debug)]
pub(crate) struct RuntimeLibrary {
    pub(crate) published: PublishedLibrary,
    graph_library: GraphLibrary,
    model_paths: MlModelPaths,
}

/// What a graph-library edit did. There is no "changed but unsaved" state:
/// the file is written before the in-memory library is adopted, so an `Err`
/// means nothing changed in memory, on disk, or in the document.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LibraryEdit {
    Committed,
    /// Nothing to publish — the node isn't a local graph instance.
    Skipped,
}

impl RuntimeLibrary {
    pub(crate) fn new(model_paths: &MlModelPaths) -> Self {
        Self::with_graph_library(model_paths, GraphLibrary::default())
    }

    pub(crate) fn load(model_paths: &MlModelPaths) -> Result<Self, GraphLibraryLoadError> {
        Ok(Self::with_graph_library(
            model_paths,
            graph_library_io::load()?,
        ))
    }

    fn with_graph_library(model_paths: &MlModelPaths, graph_library: GraphLibrary) -> Self {
        let current = Arc::new(compose(model_paths, &graph_library));
        Self {
            published: PublishedLibrary::new(current.clone()),
            graph_library,
            model_paths: model_paths.clone(),
        }
    }

    /// Add an imported template to the library. Ids are remapped so a
    /// template written elsewhere can't collide with an existing entry.
    pub(crate) fn import_template(
        &mut self,
        graph: GraphDef,
    ) -> Result<LibraryEdit, GraphLibrarySaveError> {
        let committed = graph_library_io::commit_entry(graph_library_io::LibraryEntry {
            origin: None,
            graph: graph.clone_mapped(),
        })?;
        self.adopt(committed.library);
        Ok(LibraryEdit::Committed)
    }

    /// Publish `node_id`'s local graph to the library. The file is written
    /// before anything else moves, so a failed save leaves the library, the
    /// published snapshot, and the document's lineage exactly as they were.
    pub(crate) fn publish_graph(
        &mut self,
        document: &mut Document,
        target: GraphRef,
        node_id: NodeId,
    ) -> Result<LibraryEdit, GraphLibrarySaveError> {
        let Some(publication) = publish::resolve_publication(document, target, node_id) else {
            return Ok(LibraryEdit::Skipped);
        };
        let committed = graph_library_io::commit_entry(graph_library_io::LibraryEntry {
            origin: publication.origin,
            graph: publication.graph,
        })?;
        self.adopt(committed.library);
        publish::link_origin(document, target, publication.local_id, committed.id);
        Ok(LibraryEdit::Committed)
    }

    /// Take the library the file now holds in place of our own copy, and
    /// republish the merged registry built from it.
    fn adopt(&mut self, graph_library: GraphLibrary) {
        self.graph_library = graph_library;
        self.recompose();
    }

    pub(crate) fn update_ml_model_paths(&mut self, paths: &MlModelPaths) -> bool {
        if self.model_paths == *paths {
            return false;
        }
        self.model_paths.clone_from(paths);
        self.recompose();
        true
    }

    fn recompose(&mut self) {
        let current = Arc::new(compose(&self.model_paths, &self.graph_library));
        self.published.replace(current);
    }
}

fn compose(model_paths: &MlModelPaths, graph_library: &GraphLibrary) -> ScenariumLibrary {
    let mut library = ScenariumLibrary::default();
    library.merge(math_library());
    library.merge(system_library());
    library.merge(worker_events_library());
    library.merge(fs_watch_library());
    library.merge(random_library());
    library.merge(image_library());
    library.merge(astro_library(model_paths));
    for (id, graph) in &graph_library.graphs {
        library.register_graph(*id, graph.clone_verbatim());
    }
    library
}

#[cfg(test)]
pub(crate) mod internals {
    use std::sync::Arc;

    use scenarium::Library;

    use crate::core::runtime_library::PublishedLibrary;

    pub(crate) fn published_library(library: Library) -> PublishedLibrary {
        PublishedLibrary::new(Arc::new(library))
    }

    pub(crate) fn replace(library: &PublishedLibrary, replacement: Library) {
        library.replace(Arc::new(replacement));
    }
}

#[cfg(test)]
mod tests;
