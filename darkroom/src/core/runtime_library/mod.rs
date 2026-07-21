//! The ephemeral runtime registry shared by every frontend.

use std::sync::{Arc, RwLock};

use lens::{MlModelPaths, astro_library, fs_watch_library, image_library, random_library};
use scenarium::Library as ScenariumLibrary;
use scenarium::{Graph, GraphId, math_library, system_library, worker_events_library};

use crate::core::document::Document;
use crate::core::edit::publish::{self, GraphPublicationTarget};
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

#[derive(Debug)]
pub(crate) struct RuntimeLibraryChange {
    pub(crate) changed: bool,
    pub(crate) persist_error: Option<GraphLibrarySaveError>,
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

    pub(crate) fn import_graph_template(&mut self, graph: Graph) -> RuntimeLibraryChange {
        let graph_id = GraphId::unique();
        self.persist_graph_library_change_with(
            move |library| {
                library
                    .graphs
                    .insert(graph_id, graph.fresh_copy())
                    .is_none()
            },
            graph_library_io::save,
        )
    }

    pub(crate) fn publish_graph_to_library(
        &mut self,
        document: &mut Document,
        target: GraphPublicationTarget,
    ) -> RuntimeLibraryChange {
        self.persist_graph_library_change_with(
            |library| publish::publish_graph_to_library(document, library, target),
            graph_library_io::save,
        )
    }

    fn persist_graph_library_change_with(
        &mut self,
        edit: impl FnOnce(&mut GraphLibrary) -> bool,
        persist: impl FnOnce(&GraphLibrary) -> Result<(), GraphLibrarySaveError>,
    ) -> RuntimeLibraryChange {
        let changed = edit(&mut self.graph_library);
        let persist_error = changed
            .then(|| persist(&self.graph_library))
            .and_then(Result::err);
        let outcome = RuntimeLibraryChange {
            changed,
            persist_error,
        };
        if outcome.changed {
            self.recompose();
        }
        outcome
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
        library.insert_graph(*id, graph.clone());
    }
    library
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod test_support {
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
