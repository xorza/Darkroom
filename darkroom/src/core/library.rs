//! The runtime function library, shared by every frontend.

use lens::{astro_library, fs_watch_library, image_library, random_library};
use scenarium::Library;
use scenarium::math_library;
use scenarium::system_library;
use scenarium::worker_events_library;

/// Assemble the built-in runtime function library. Builtins carry no
/// graphs — the on-disk graph library is folded in by
/// [`Engine::new`](crate::core::engine::Engine), which owns the library
/// file's whole lifecycle (load outcome reporting, persist-on-edit).
pub(crate) fn runtime_func_lib() -> Library {
    let mut library = Library::default();
    library.merge(math_library());
    library.merge(system_library());
    library.merge(worker_events_library());
    library.merge(fs_watch_library());
    library.merge(random_library());
    library.merge(image_library());
    library.merge(astro_library());
    library
}

#[cfg(test)]
mod tests {
    use crate::core::library::runtime_func_lib;

    #[test]
    fn runtime_library_includes_lens_utilities() {
        let library = runtime_func_lib();

        assert!(library.by_name("Watch Directory").is_some());
        assert!(library.by_name("Random").is_some());
    }
}
