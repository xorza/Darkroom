//! Application-level node-function libraries for `scenarium`: filesystem and
//! random utilities plus `imaginarium` image operations and `lumos` astro
//! processing. `config_node` is the shared `common::Introspect` →
//! config-builder bridge.

mod astro;
mod config_node;
mod fs_watch_library;
mod image;
mod random_library;

// Published surface — only what darkroom consumes. Everything else (config
// mirrors, presets, datatypes, the config bridge) stays crate-internal.
pub use astro::library::{MlModelPaths, astro_library, ml_model_paths, set_ml_model_paths};
pub use fs_watch_library::fs_watch_library;
pub use image::library::image_library;
pub use image::{IMAGE_TYPE_ID, Image};
pub use random_library::random_library;
