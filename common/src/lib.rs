//! Shared leaf utilities: the pieces more than one crate in the workspace
//! needs and none of them owns. It depends on nothing in-tree, so everything
//! here has to make sense without knowing what a node graph or an image is.
//!
//! The published surface is narrower than the export list looks, because most
//! of those names are *reached through* a handful of entry points rather than
//! imported:
//!
//! - [`CancelToken`] — cooperative cancellation, shared across worker threads.
//! - [`serialize`] / [`deserialize`] over a [`SerdeFormat`] — the one
//!   format-tagged codec every document and sidecar in the workspace is
//!   written with; [`serialize_into`] appends to a buffer the caller keeps.
//!   [`SerializeError`] and [`DeserializeError`] appear in those signatures;
//!   nothing imports them to construct one.
//! - [`Introspect`] / [`IntrospectEnum`] and the [`FieldDesc`] … [`FieldValue`]
//!   vocabulary — generic struct description, which is how a config struct
//!   becomes editor UI. Under the `introspect-derive` feature the matching
//!   derives expand to `::common::…` paths through every one of them, so
//!   [`IntrospectInteger`], [`IntrospectFloat`] and [`IntrospectError`] are
//!   exported for generated code to name rather than for hand-written `use`s.
//! - [`file_utils`] — atomic same-directory publication, and [`FileIdentity`].
//!
//! [`FloatExt`], [`is_debug`] and [`id_type!`] stand on their own. `TempDir`,
//! `TempFile` and the `internals` module are test scaffolding, gated behind
//! the `internals` feature so they never enter a release build.

#![deny(unsafe_code)]
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

// Type-holding modules are `pub(crate)`; their public surface is defined by the
// crate-root `pub use`s below (one canonical path per item). Modules that are
// free-function namespaces (or a macro home) stay `pub` and are used as
// `common::<module>::fn`.

#[macro_use]
pub mod macros;
pub mod file_utils;
#[cfg(any(test, feature = "internals"))]
pub mod internals;

pub(crate) mod cancel_token;
pub(crate) mod float_ext;
pub(crate) mod introspect;
pub(crate) mod serde;

pub use cancel_token::CancelToken;
pub use file_utils::file_identity::FileIdentity;
pub use float_ext::FloatExt;
#[cfg(any(test, feature = "internals"))]
pub use internals::temp_dir::TempDir;
#[cfg(any(test, feature = "internals"))]
pub use internals::temp_file::TempFile;
#[cfg(all(unix, any(test, feature = "internals")))]
pub use internals::unreadable::Unreadable;
pub use introspect::{
    FieldDesc, FieldKind, FieldValue, FloatKind, IntegerKind, IntegerValue, Introspect,
    IntrospectEnum, IntrospectError, IntrospectFloat, IntrospectInteger,
};
pub use serde::serde_format::SerdeFormat;
pub use serde::{DeserializeError, SerializeError, deserialize, serialize, serialize_into};

/// Whether this build has debug assertions on — the one switch every
/// debug-only self-check in the workspace is gated on, so those checks turn
/// on and off together instead of each crate reading the flag its own way.
pub const fn is_debug() -> bool {
    cfg!(debug_assertions)
}

// Lets `common-derive`'s generated `::common::…` paths resolve inside `common`, whose only derive
// users are its own tests.
#[cfg(test)]
extern crate self as common;
