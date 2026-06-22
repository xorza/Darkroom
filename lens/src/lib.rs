//! `lens` — node-function library adapting `imaginarium` (GPU image ops) and
//! `lumos` (astro processing) into the `scenarium` node graph. Two domains live
//! under `image/` and `astro/`; `config_node` is the shared
//! `common::Introspect` → config-builder bridge.

mod astro;
mod config_node;
mod image;

// Published surface — only what darkroom consumes. Everything else (config
// mirrors, presets, datatypes, the config bridge) stays crate-internal.
pub use astro::funclib::astro_funclib;
pub use image::Image;
pub use image::funclib::image_funclib;
