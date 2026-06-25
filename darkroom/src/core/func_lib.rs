//! The runtime function library, shared by every frontend.

use std::sync::Arc;

use arc_swap::ArcSwap;
use lens::{astro_funclib, image_funclib, register_image_codec};
use scenarium::elements::basic_funclib::basic_funclib;
use scenarium::elements::worker_events_funclib::worker_events_funclib;
use scenarium::prelude::{CustomValueRegistry, FuncLib};

use crate::core::io::library;

/// The runtime library behind a swappable cell (see [`Engine::func_lib`]).
/// The `ArcSwap` lets promote/publish atomically swap in a grown copy that
/// every holder picks up on its next `load`; the outer `Arc` shares the
/// *same* slot with the script host rather than a frozen snapshot.
pub(crate) type SharedFuncLib = Arc<ArcSwap<FuncLib>>;

/// Assemble the runtime function library — builtins plus the on-disk
/// subgraph library — into the shared swappable cell. Builtins carry no
/// subgraph defs, so `func_lib.subgraphs` *is* the shared subgraph library:
/// loaded from the library file here at startup, grown by "promote".
pub(crate) fn runtime_func_lib() -> SharedFuncLib {
    let mut func_lib = FuncLib::default();
    func_lib.merge(basic_funclib());
    func_lib.merge(worker_events_funclib());
    func_lib.merge(image_funclib());
    func_lib.merge(astro_funclib());
    for def in library::load_library() {
        func_lib.add_subgraph(def);
    }
    Arc::new(ArcSwap::from_pointee(func_lib))
}

/// The custom-value codecs the disk cache needs to serialize a `persist`
/// node's outputs to/from blobs. Mirrors the custom types our funclibs
/// produce — currently just `lens`' image type. A node whose output type
/// has no codec here is silently skipped by the cache (recomputed each run),
/// never an error.
pub(crate) fn runtime_codec_registry() -> CustomValueRegistry {
    let mut registry = CustomValueRegistry::default();
    register_image_codec(&mut registry);
    registry
}
