//! The ephemeral runtime registry shared by every frontend.

use std::sync::{Arc, RwLock};

use lens::{MlModelPaths, astro_library, fs_watch_library, image_library, random_library};
use scenarium::Library as ScenariumLibrary;
use scenarium::{math_library, system_library, worker_events_library};

use crate::core::preview::{self, PreviewSink};

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
    model_paths: MlModelPaths,
    /// Where the preview func's lambda publishes. Created once and captured by
    /// every recomposed snapshot, so the values in flight survive a settings
    /// change rebuilding the registry underneath them.
    pub(crate) previews: Arc<PreviewSink>,
}

impl RuntimeLibrary {
    pub(crate) fn new(model_paths: &MlModelPaths) -> Self {
        let previews = Arc::new(PreviewSink::default());
        let current = Arc::new(compose(model_paths, &previews));
        Self {
            published: PublishedLibrary::new(Arc::clone(&current)),
            model_paths: model_paths.clone(),
            previews,
        }
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
        let current = Arc::new(compose(&self.model_paths, &self.previews));
        self.published.replace(current);
    }
}

fn compose(model_paths: &MlModelPaths, previews: &Arc<PreviewSink>) -> ScenariumLibrary {
    let mut library = ScenariumLibrary::default();
    library.add(preview::preview_func(Arc::clone(previews)));
    library.merge(math_library());
    library.merge(system_library());
    library.merge(worker_events_library());
    library.merge(fs_watch_library());
    library.merge(random_library());
    library.merge(image_library());
    library.merge(astro_library(model_paths));
    library
}
