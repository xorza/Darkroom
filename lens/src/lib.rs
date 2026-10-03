//! Application-level node-function libraries for `scenarium`: filesystem and
//! random utilities plus `imaginarium` image operations and `lumos` astro
//! processing. `config_node` is the shared `common::Introspect` →
//! config-builder bridge.

#![forbid(unsafe_code)]
// Lints of the workspace set that stay `allow` there until lumos is swept (plan 2.2);
// this crate is clean for them, so they warn here.
#![warn(
    unused_macro_rules,
    clippy::allow_attributes_without_reason,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::ignore_without_reason,
    clippy::items_after_statements,
    clippy::large_stack_arrays,
    clippy::large_types_passed_by_value,
    clippy::let_underscore_must_use,
    clippy::map_err_ignore,
    clippy::match_same_arms,
    clippy::missing_fields_in_debug,
    clippy::needless_pass_by_value,
    clippy::print_stderr,
    clippy::print_stdout,
    clippy::should_panic_without_expect,
    clippy::struct_field_names,
    clippy::trivially_copy_pass_by_ref,
    clippy::unused_result_ok,
    clippy::unused_self
)]

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
