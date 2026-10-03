//! Application-level node-function libraries for `scenarium`: filesystem and
//! random utilities plus `imaginarium` image operations and `lumos` astro
//! processing. `config_node` is the shared `common::Introspect` →
//! config-builder bridge.

#![forbid(unsafe_code)]
// Lints of the workspace set that stay `allow` there until lumos is swept (plan 2.2);
// this crate is clean for them, so they warn here.
#![warn(clippy::cast_possible_wrap, clippy::cast_sign_loss)]

mod astro;
mod config_node;
mod image;
mod utility;

// Published surface — only what darkroom consumes. Everything else (config
// projections, presets, datatypes, the config bridge) stays crate-internal.
pub use astro::nodes::{MlModelPaths, astro_library};
pub use image::nodes::image_library;
pub use image::{IMAGE_TYPE_ID, Image};
pub use utility::fs_watch::fs_watch_library;
pub use utility::random::random_library;
