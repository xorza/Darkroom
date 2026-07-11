//! The runtime function library, shared by every frontend.

use lens::{astro_library, image_library};
use scenarium::elements::fs_watch_library::fs_watch_library;
use scenarium::elements::math_library::math_library;
use scenarium::elements::system_library::system_library;
use scenarium::elements::worker_events_library::worker_events_library;
use scenarium::library::Library;

/// Assemble the built-in runtime function library. Builtins carry no
/// subgraph defs — the on-disk subgraph library is folded in by
/// [`Engine::new`](crate::core::engine::Engine), which owns the library
/// file's whole lifecycle (load outcome reporting, persist-on-edit).
pub(crate) fn runtime_func_lib() -> Library {
    let mut library = Library::default();
    library.merge(math_library());
    library.merge(system_library());
    library.merge(worker_events_library());
    library.merge(fs_watch_library());
    library.merge(image_library());
    library.merge(astro_library());
    library
}
