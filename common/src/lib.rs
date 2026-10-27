//! Shared leaf utilities: the pieces more than one crate in the workspace
//! needs and none of them owns. It depends on nothing in-tree, so everything
//! here has to make sense without knowing what a node graph or an image is.
//!
//! The published surface is narrower than the export list looks, because most
//! of those names are *reached through* a handful of entry points rather than
//! imported:
//!
//! - [`CancelToken`] — cooperative cancellation, shared across worker threads.
//! - [`SerdeFormat`] and its [`serialize`](SerdeFormat::serialize) /
//!   [`deserialize`](SerdeFormat::deserialize) — the one format-tagged codec
//!   every document and sidecar in the workspace is written with;
//!   [`serialize_into`](SerdeFormat::serialize_into) appends to a buffer the
//!   caller keeps.
//!   [`SerializeError`] and [`DeserializeError`] appear in those signatures;
//!   nothing imports them to construct one.
//! - [`Introspect`] / [`IntrospectEnum`] and the [`FieldDesc`] … [`FieldValue`]
//!   vocabulary — generic struct description, which is how a config struct
//!   becomes editor UI. Under the `introspect-derive` feature the matching
//!   derives expand to `::common::…` paths through every one of them, so
//!   [`IntrospectInteger`], [`IntrospectFloat`] and [`IntrospectError`] are
//!   exported for generated code to name rather than for hand-written `use`s.
//! - The one route outside `common` to what the program does beyond its own memory:
//!   [`fs`] for the file system, with atomic publication, [`FileIdentity`], [`TempDir`] and,
//!   beside it, [`MappedFile`] and [`ScratchFile`]; [`env`](mod@env) for the environment;
//!   [`process`] for other processes; [`thread`], [`task`] and [`BgRuntime`] for a new
//!   thread or runtime; [`par`] for parallel work; [`SystemMemory`], [`ResidentMemory`],
//!   [`FileSystem`] and [`UserCache`] for what the system states. The workspace's `clippy.toml`
//!   refuses the direct calls everywhere else.
//! - Parallel work: [`par`] starts every parallel loop, and panics when a loop's body would start
//!   another; [`PerWorker`] is the scratch such a body takes.
//!
//! [`Id`], [`FloatExt`] and [`is_debug`] stand on their own, as do [`Buffer2`], the row-major 2D
//! grid, and its geometry: [`Size2us`], [`Vec2us`] and [`Rect2us`].
//! `TempFile` and the `internals` module are test scaffolding, gated behind
//! the `internals` feature so they never enter a release build, as is `bench`, the policy every
//! crate's benches share: a case's kind and the sampling of its group.

#![deny(unsafe_code)]
// The one `thread_local!` is test-only: `WorkerPool` marks the worker whose body is top-level code.
// An item-position macro's lint level is read at the crate root.
#![cfg_attr(
    feature = "internals",
    expect(
        clippy::disallowed_types,
        reason = "`WorkerPool` marks its top-level worker in a thread-local: rayon runs the body, \
                  with no owner to carry the mark"
    )
)]

// Type-holding modules are private; their public surface is defined by the
// crate-root `pub use`s below (one canonical path per item). Modules that are
// free-function namespaces stay `pub` and are used as `common::<module>::fn`.

#[cfg(feature = "internals")]
pub mod bench;
pub mod env;
pub mod fs;
#[cfg(feature = "internals")]
pub mod internals;
pub mod par;
pub mod process;
pub mod task;
pub mod thread;

mod bg_runtime;
mod buffer2;
mod cancel_token;
mod float_ext;
mod id;
mod introspect;
#[expect(unsafe_code, reason = "memory maps and the OS calls behind them")]
mod platform;
mod rect2us;
mod serde;
mod size2us;
mod vec2us;

pub use bg_runtime::BgRuntime;
pub use buffer2::Buffer2;
pub use cancel_token::CancelToken;
pub use float_ext::FloatExt;
pub use fs::file_identity::{FileId, FileIdentity};
pub use fs::temp_dir::TempDir;
pub use id::Id;
#[cfg(feature = "internals")]
pub use internals::temp_file::TempFile;
#[cfg(feature = "internals")]
pub use internals::unreadable::Unreadable;
pub use platform::{
    Eviction, FileSystem, MappedFile, ResidentMemory, ScratchFile, SystemMemory, UserCache,
};
// The derives share their traits' names in the macro namespace, so one name exports both.
#[cfg(feature = "introspect-derive")]
pub use common_derive::{Introspect, IntrospectEnum};
pub use introspect::{
    FieldDesc, FieldKind, FieldValue, FloatKind, IntegerKind, IntegerValue, Introspect,
    IntrospectEnum, IntrospectError, IntrospectFloat, IntrospectInteger,
};
pub use par::per_worker::PerWorker;
#[cfg(feature = "internals")]
pub use par::worker_pool::WorkerPool;
pub use rect2us::Rect2us;
pub use serde::hex_bytes;
pub use serde::serde_format::SerdeFormat;
pub use serde::{DeserializeError, SerializeError};
pub use size2us::Size2us;
pub use vec2us::Vec2us;

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
