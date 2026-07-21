//! The runtime function library, shared by every frontend.

use std::sync::{Arc, RwLock};

use lens::{MlModelPaths, astro_library, fs_watch_library, image_library, random_library};
use scenarium::Library as ScenariumLibrary;
use scenarium::math_library;
use scenarium::system_library;
use scenarium::worker_events_library;

#[derive(Clone, Debug)]
pub(crate) struct PublishedLibrary {
    current: Arc<RwLock<Arc<ScenariumLibrary>>>,
}

impl PublishedLibrary {
    pub(crate) fn new(current: Arc<ScenariumLibrary>) -> Self {
        Self {
            current: Arc::new(RwLock::new(current)),
        }
    }

    pub(crate) fn load(&self) -> Arc<ScenariumLibrary> {
        self.current.read().unwrap().clone()
    }

    pub(crate) fn replace(&self, current: Arc<ScenariumLibrary>) {
        *self.current.write().unwrap() = current;
    }
}

#[derive(Debug)]
pub(crate) struct RuntimeLibrary {
    pub(crate) current: Arc<ScenariumLibrary>,
    pub(crate) published: PublishedLibrary,
    model_paths: MlModelPaths,
}

impl RuntimeLibrary {
    pub(crate) fn new(model_paths: &MlModelPaths) -> Self {
        let mut current = ScenariumLibrary::default();
        current.merge(math_library());
        current.merge(system_library());
        current.merge(worker_events_library());
        current.merge(fs_watch_library());
        current.merge(random_library());
        current.merge(image_library());
        current.merge(astro_library(model_paths));
        let current = Arc::new(current);
        Self {
            published: PublishedLibrary::new(current.clone()),
            current,
            model_paths: model_paths.clone(),
        }
    }

    pub(crate) fn edit(&mut self, edit: impl FnOnce(&mut ScenariumLibrary) -> bool) -> bool {
        let changed = edit(Arc::make_mut(&mut self.current));
        if changed {
            self.publish();
        }
        changed
    }

    pub(crate) fn update_ml_model_paths(&mut self, paths: &MlModelPaths) -> bool {
        if self.model_paths == *paths {
            return false;
        }
        lens::configure_ml_model_defaults(Arc::make_mut(&mut self.current), paths);
        self.model_paths.clone_from(paths);
        self.publish();
        true
    }

    fn publish(&self) {
        self.published.replace(self.current.clone());
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use lens::MlModelPaths;
    use scenarium::StaticValue;

    use crate::core::library::RuntimeLibrary;

    #[test]
    fn runtime_library_includes_lens_utilities_and_updates_ml_defaults() {
        let defaults = MlModelPaths::default();
        let mut library = RuntimeLibrary::new(&defaults);

        assert!(library.current.by_name("Watch Directory").is_some());
        assert!(library.current.by_name("Random").is_some());
        assert!(!library.update_ml_model_paths(&defaults));

        let paths = MlModelPaths {
            denoise: PathBuf::from("/models/denoise.onnx"),
            star_removal: PathBuf::from("/models/stars.onnx"),
        };
        assert!(library.update_ml_model_paths(&paths));
        let published = library.published.load();
        assert_eq!(
            published.by_name("ML Denoise").unwrap().inputs[1].default_value,
            Some(StaticValue::FsPath(paths.denoise.display().to_string()))
        );
        assert_eq!(
            library.current.by_name("ML Star Removal").unwrap().inputs[1].default_value,
            Some(StaticValue::FsPath(
                paths.star_removal.display().to_string()
            ))
        );
    }
}
