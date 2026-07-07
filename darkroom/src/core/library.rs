//! The runtime function library, shared by every frontend.

use std::sync::Arc;

use arc_swap::ArcSwap;
use lens::{astro_library, image_library};
use scenarium::elements::fs_watch_library::fs_watch_library;
use scenarium::elements::math_library::math_library;
use scenarium::elements::system_library::system_library;
use scenarium::elements::worker_events_library::worker_events_library;
use scenarium::library::Library;

use crate::core::io::library;

/// The runtime library behind a swappable cell (see [`Engine::library`]).
/// The `ArcSwap` lets promote/publish atomically swap in a grown copy that
/// every holder picks up on its next `load`; the outer `Arc` shares the
/// *same* slot with the script host rather than a frozen snapshot.
pub(crate) type SharedLibrary = Arc<ArcSwap<Library>>;

/// Assemble the runtime function library — builtins plus the on-disk
/// subgraph library — into the shared swappable cell. Builtins carry no
/// subgraph defs, so `library.subgraphs` *is* the shared subgraph library:
/// loaded from the library file here at startup, grown by "promote".
pub(crate) fn runtime_func_lib() -> SharedLibrary {
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
    Arc::new(ArcSwap::from_pointee(library))
}
