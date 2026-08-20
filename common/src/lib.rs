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
//!   written with. [`SerializeError`], [`DeserializeError`], [`Lz4SizeError`],
//!   [`FileExtensionError`] and [`FileFormatResult`] appear in those
//!   signatures; nothing imports them to construct one.
//! - [`Introspect`] / [`IntrospectEnum`] and the [`FieldDesc`] … [`FieldValue`]
//!   vocabulary — generic struct description, which is how a config struct
//!   becomes editor UI. Under the `introspect-derive` feature the matching
//!   derives expand to `::common::…` paths through every one of them, so
//!   [`IntrospectInteger`], [`IntrospectFloat`] and [`IntrospectError`] are
//!   exported for generated code to name rather than for hand-written `use`s.
//! - [`file_utils`] — file discovery and atomic same-directory publication.
//!
//! [`FloatExt`], [`is_debug`] and [`id_type!`] stand on their own. `TempDir`,
//! `TempFile` and the `internals` module are test scaffolding, gated behind
//! the `internals` feature so they never enter a release build.

// Type-holding modules are `pub(crate)`; their public surface is defined by the
// crate-root `pub use`s below (one canonical path per item). Modules that are
// free-function namespaces (or a macro home) stay `pub` and are used as
// `common::<module>::fn`.

// Lets `common-derive`'s generated `::common::…` paths resolve inside `common`
// itself (e.g. its own `#[derive(Introspect)]` test).
extern crate self as common;

#[macro_use]
pub mod macros;
pub mod file_utils;
#[cfg(any(test, feature = "internals"))]
pub mod internals;
pub mod serde;

pub(crate) mod cancel_token;
pub(crate) mod file_format;
pub(crate) mod float_ext;
pub(crate) mod introspect;

pub use cancel_token::CancelToken;
pub use file_format::{FileExtensionError, FileFormatResult, SerdeFormat};
pub use float_ext::FloatExt;
#[cfg(any(test, feature = "internals"))]
pub use internals::temp_dir::TempDir;
#[cfg(any(test, feature = "internals"))]
pub use internals::temp_file::TempFile;
pub use introspect::{
    FieldDesc, FieldKind, FieldValue, FloatKind, IntegerKind, IntegerValue, Introspect,
    IntrospectEnum, IntrospectError, IntrospectFloat, IntrospectInteger,
};
pub use serde::{DeserializeError, Lz4SizeError, SerializeError, deserialize, serialize};

/// Whether this build has debug assertions on — the one switch every
/// debug-only self-check in the workspace is gated on, so those checks turn
/// on and off together instead of each crate reading the flag its own way.
pub const fn is_debug() -> bool {
    cfg!(debug_assertions)
}
