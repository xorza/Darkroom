//! The ephemeral runtime registry shared by every frontend.

use std::path::{self, PathBuf};
use std::sync::Arc;

use lens::{MlModelPaths, astro_library, fs_watch_library, image_library, random_library};
use scenarium::Library as ScenariumLibrary;
use scenarium::{math_library, system_library, worker_events_library};

use crate::core::preview_sink::{self, PreviewSink};

#[derive(Debug)]
pub(crate) struct RuntimeLibrary {
    /// The registry as last composed. Replaced whole on a recompose, never edited.
    cur: ScenariumLibrary,
    model_paths: MlModelPaths,
    /// Where the preview func's lambda publishes. Captured by every recomposed snapshot, so
    /// values in flight survive a settings change rebuilding the registry.
    pub(crate) previews: Arc<PreviewSink>,
}

impl RuntimeLibrary {
    pub(crate) fn new(model_paths: &MlModelPaths) -> Self {
        let previews = Arc::new(PreviewSink::default());
        Self {
            cur: compose(model_paths, &previews),
            model_paths: model_paths.clone(),
            previews,
        }
    }

    pub(crate) fn set_ml_model_paths(&mut self, paths: &MlModelPaths) {
        if self.model_paths == *paths {
            return;
        }
        self.model_paths.clone_from(paths);
        self.recompose();
    }

    fn recompose(&mut self) {
        self.cur = compose(&self.model_paths, &self.previews);
    }

    /// The registry as last composed.
    pub(crate) const fn cur(&self) -> &ScenariumLibrary {
        &self.cur
    }
}

fn compose(model_paths: &MlModelPaths, previews: &Arc<PreviewSink>) -> ScenariumLibrary {
    // A model path binds into each new node as its default, and a document holds absolute paths:
    // a relative one is taken against the working directory, where a run would have read it.
    let absolute = |path: &PathBuf| path::absolute(path).unwrap_or_else(|_| path.clone());
    let model_paths = MlModelPaths {
        denoise: absolute(&model_paths.denoise),
        star_removal: absolute(&model_paths.star_removal),
    };
    let mut library = ScenariumLibrary::default();
    library.add(preview_sink::func(Arc::clone(previews)));
    library.merge(math_library());
    library.merge(system_library());
    library.merge(worker_events_library());
    library.merge(fs_watch_library());
    library.merge(random_library());
    library.merge(image_library());
    library.merge(astro_library(&model_paths));
    library
}

#[cfg(test)]
mod tests {
    use std::path::{self, PathBuf};
    use std::sync::Arc;

    use lens::{MlModelPaths, astro_library, fs_watch_library, image_library, random_library};
    use scenarium::{math_library, system_library, worker_events_library};

    use crate::core::runtime_library::compose;

    /// Every shipped func and type id is distinct across the merged libraries: `merge` refuses a
    /// repeated id, so composing them all is the check, and the counts show none was dropped. The
    /// ids are constants saved graphs bind to, so a collision would load one node as another.
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
            library.iter().count(),
            preview + parts.iter().map(|part| part.iter().count()).sum::<usize>()
        );
        assert_eq!(
            library.types().len(),
            parts.iter().map(|part| part.types().len()).sum::<usize>()
        );
    }

    /// The default model paths are bare file names; each path default the library binds into a
    /// new node is absolute, taken against the working directory, as a document holds its paths.
    #[test]
    fn the_model_path_defaults_are_absolute() {
        let paths = MlModelPaths::default();
        assert!(paths.denoise.is_relative() && paths.star_removal.is_relative());
        let library = compose(&paths, &Arc::default());
        let mut defaults: Vec<PathBuf> = library
            .iter()
            .flat_map(|func| func.inputs().iter())
            .filter_map(|input| {
                input
                    .default_value
                    .as_ref()?
                    .as_fs_path()
                    .map(PathBuf::from)
            })
            .collect();
        defaults.sort();
        let mut expected = vec![
            path::absolute(&paths.denoise).unwrap(),
            path::absolute(&paths.star_removal).unwrap(),
        ];
        expected.sort();
        assert_eq!(defaults, expected);
    }
}
