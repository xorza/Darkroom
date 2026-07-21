//! Application-level node-function libraries for `scenarium`: filesystem and
//! random utilities plus `imaginarium` image operations and `lumos` astro
//! processing. `config_node` is the shared `common::Introspect` →
//! config-builder bridge.

mod astro;
mod config_node;
mod image;
mod utility;

// Published surface — only what darkroom consumes. Everything else (config
// mirrors, presets, datatypes, the config bridge) stays crate-internal.
pub use astro::nodes::{MlModelPaths, astro_library, configure_ml_model_defaults};
pub use image::nodes::image_library;
pub use image::{IMAGE_TYPE_ID, Image};
pub use utility::fs_watch::fs_watch_library;
pub use utility::random::random_library;
