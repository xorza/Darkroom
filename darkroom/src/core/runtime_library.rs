//! The ephemeral runtime registry shared by every frontend.

use std::path::{self, PathBuf};
use std::sync::Arc;

use lens::{MlModelPaths, astro_library, fs_watch_library, image_library, random_library};
use scenarium::{Library as ScenariumLibrary, LibraryBuilder};
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
    let absolute = |path: &Option<PathBuf>| {
        path.as_ref()
            .map(|path| path::absolute(path).unwrap_or_else(|_| path.clone()))
    };
    let model_paths = MlModelPaths {
        denoise: absolute(&model_paths.denoise),
        star_removal: absolute(&model_paths.star_removal),
    };
    let mut library = LibraryBuilder::default();
    preview_sink::register(&mut library, Arc::clone(previews));
    library.merge(math_library());
    library.merge(system_library());
    library.merge(worker_events_library());
    library.merge(fs_watch_library());
    library.merge(random_library());
    library.merge(image_library());
    library.merge(astro_library(&model_paths));
    library.build()
}

#[cfg(test)]
mod tests {
    use std::path::{self, PathBuf};
    use std::sync::Arc;

    use lens::{MlModelPaths, astro_library, fs_watch_library, image_library, random_library};
    use scenarium::{
        DataType, LibraryBuilder, math_library, system_library, worker_events_library,
    };

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
        // The preview func, and the enum type of its Display.
        let (preview, display) = (1, 1);
        assert_eq!(
            library.iter().count(),
            preview + parts.iter().map(LibraryBuilder::func_count).sum::<usize>()
        );
        assert_eq!(
            library.type_count(),
            display + parts.iter().map(LibraryBuilder::type_count).sum::<usize>()
        );
    }

    /// Every type name a person reads is Title Case: each word starts with a capital or a digit,
    /// and none holds a lower-case letter before a capital, the mark of a Rust identifier. The
    /// names checked are the built-ins', every custom and enum type's, and every file kind a port
    /// takes. The failure lists each name that breaks the rule.
    #[test]
    fn every_type_name_is_title_case() {
        let library = compose(&MlModelPaths::default(), &Arc::default());
        let ports = library.iter().flat_map(|func| {
            let inputs = func.inputs().iter().map(|input| input.data_type);
            inputs.chain(func.outputs().iter().map(|output| output.data_type))
        });
        let mut names: Vec<&str> = [
            DataType::Any,
            DataType::Float,
            DataType::Int,
            DataType::Bool,
            DataType::String,
            DataType::Dir,
        ]
        .into_iter()
        .chain(library.data_types().iter().copied())
        .chain(ports)
        .map(|data_type| library.type_name(data_type))
        .collect();
        names.sort_unstable();
        names.dedup();
        assert!(names.contains(&"Integer"), "{names:?}");
        assert!(names.contains(&"Image"), "{names:?}");
        let title_case = |name: &str| {
            name.split(' ').all(|word| {
                let starts = word
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_uppercase() || c.is_ascii_digit());
                let hump = word
                    .chars()
                    .zip(word.chars().skip(1))
                    .any(|(a, b)| a.is_lowercase() && b.is_uppercase());
                starts && !hump
            })
        };
        let broken: Vec<&str> = names.into_iter().filter(|name| !title_case(name)).collect();
        assert_eq!(broken, Vec::<&str>::new());
    }

    /// No model is set by default, so no input of the library declares a path default: a new ML
    /// node starts at the empty path its pick button fills. A model set by a relative path binds
    /// into a new node absolute, taken against the working directory, as a document holds its
    /// paths; one left unset still declares nothing.
    #[test]
    fn a_model_path_default_is_absolute_and_an_unset_one_is_none() {
        let path_defaults = |paths: &MlModelPaths| -> Vec<PathBuf> {
            let library = compose(paths, &Arc::default());
            library
                .iter()
                .flat_map(|func| func.inputs().iter())
                .filter_map(|input| {
                    input
                        .default_value
                        .as_ref()?
                        .as_fs_path()
                        .map(PathBuf::from)
                })
                .collect()
        };
        assert_eq!(
            path_defaults(&MlModelPaths::default()),
            Vec::<PathBuf>::new()
        );
        let denoise = PathBuf::from("denoise.onnx");
        let paths = MlModelPaths {
            denoise: Some(denoise.clone()),
            star_removal: None,
        };
        assert_eq!(path_defaults(&paths), [path::absolute(&denoise).unwrap()]);
    }
}
