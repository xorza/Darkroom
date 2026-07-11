//! The runtime function library, shared by every frontend.

use std::sync::Arc;

use lens::{astro_library, image_library};
use scenarium::elements::fs_watch_library::fs_watch_library;
use scenarium::elements::math_library::math_library;
use scenarium::elements::system_library::system_library;
use scenarium::elements::worker_events_library::worker_events_library;
use scenarium::library::Library;

use crate::core::io::library;

/// Assemble the runtime function library — builtins plus the on-disk
/// subgraph library. Builtins carry no subgraph defs, so `library.subgraphs`
/// *is* the shared subgraph library: loaded from the library file here at
/// startup, grown by "promote" (`Arc::make_mut` on the engine's handle —
/// see [`Engine::library`](crate::core::engine::Engine) for the sharing
/// story).
pub(crate) fn runtime_func_lib() -> Arc<Library> {
    let mut library = Library::default();
    library.merge(math_library());
    library.merge(system_library());
    library.merge(worker_events_library());
    library.merge(fs_watch_library());
    library.merge(image_library());
    library.merge(astro_library());
    for def in library::load_library() {
        library.add_subgraph(def);
    }
    Arc::new(library)
}
