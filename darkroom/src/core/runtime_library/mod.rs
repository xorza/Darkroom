//! The ephemeral runtime registry shared by every frontend.

use std::sync::Arc;

use lens::{MlModelPaths, astro_library, fs_watch_library, image_library, random_library};
use scenarium::Library as ScenariumLibrary;
use scenarium::{math_library, system_library, worker_events_library};

use crate::core::preview::{self, PreviewSink};

#[derive(Debug)]
pub(crate) struct RuntimeLibrary {
    /// The registry as last composed. Replaced whole on a recompose, never
    /// edited, so a frame that cloned the `Arc` keeps reading one registry
    /// throughout.
    current: Arc<ScenariumLibrary>,
    model_paths: MlModelPaths,
    /// Where the preview func's lambda publishes. Created once and captured by
    /// every recomposed snapshot, so the values in flight survive a settings
    /// change rebuilding the registry underneath them.
    pub(crate) previews: Arc<PreviewSink>,
}

impl RuntimeLibrary {
    pub(crate) fn new(model_paths: &MlModelPaths) -> Self {
        let previews = Arc::new(PreviewSink::default());
        Self {
            current: Arc::new(compose(model_paths, &previews)),
            model_paths: model_paths.clone(),
            previews,
        }
    }

    pub(crate) fn update_ml_model_paths(&mut self, paths: &MlModelPaths) {
        if self.model_paths == *paths {
            return;
        }
        self.model_paths.clone_from(paths);
        self.recompose();
    }

    fn recompose(&mut self) {
        self.current = Arc::new(compose(&self.model_paths, &self.previews));
    }

    /// The registry as last composed.
    pub(crate) fn current(&self) -> &Arc<ScenariumLibrary> {
        &self.current
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use lens::{MlModelPaths, astro_library, fs_watch_library, image_library, random_library};
    use scenarium::{math_library, system_library, worker_events_library};

    use crate::core::runtime_library::compose;

    /// Every shipped func and type id is distinct across all the libraries the editor
    /// merges: `merge` refuses a repeated id, so composing them all is the check, and the
    /// counts show no library was dropped along the way. The ids are hand-assigned
    /// constants that saved graphs bind to, so a collision would make one node load as
    /// another.
    #[test]
    fn every_func_and_type_id_is_unique_across_the_libraries() {
        let paths = MlModelPaths::default();
        let parts = [
            math_library(),
            system_library(),
            worker_events_library(),
            fs_watch_library(),
            random_library(),
            image_library(),
            astro_library(&paths),
        ];
        let library = compose(&paths, &Arc::default());
        let preview = 1;
        assert_eq!(
            library.funcs().count(),
            preview + parts.iter().map(|part| part.funcs().count()).sum::<usize>()
        );
        assert_eq!(
            library.types().len(),
            parts.iter().map(|part| part.types().len()).sum::<usize>()
        );
    }
}
