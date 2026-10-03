//! Application-level node-function libraries for `scenarium`: filesystem and
//! random utilities plus `imaginarium` image operations and `lumos` astro
//! processing. `config_node` is the shared `common::Introspect` →
//! config-builder bridge.

#![forbid(unsafe_code)]

mod astro;
mod config_node;
mod fs_watch;
mod image;
mod random;

// Published surface — only what darkroom consumes. Everything else (config
// projections, presets, datatypes, the config bridge) stays crate-internal.
pub use astro::nodes::{MlModelPaths, astro_library};
pub use fs_watch::fs_watch_library;
pub use image::nodes::image_library;
pub use image::{IMAGE_TYPE_ID, Image};
pub use random::random_library;
