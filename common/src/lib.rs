// Type-holding modules are `pub(crate)`; their public surface is defined by the
// crate-root `pub use`s below (one canonical path per item). Modules that are
// free-function namespaces (or a macro home) stay `pub` and are used as
// `common::<module>::fn`.

// Lets `common-derive`'s generated `::common::…` paths resolve inside `common`
// itself (e.g. its own `#[derive(Introspect)]` test).
extern crate self as common;

#[macro_use]
pub mod macros;
pub mod cpu_features;
pub mod file_utils;
pub mod parallel;
pub mod serde;
#[cfg(any(test, feature = "test-support"))]
pub mod test_utils;

pub(crate) mod bit_buffer2;
pub(crate) mod cancel_token;
pub(crate) mod constants;
pub(crate) mod debug;
pub(crate) mod file_format;
pub(crate) mod float_ext;
pub(crate) mod fnv;
pub(crate) mod introspect;
pub(crate) mod key_index_vec;
pub(crate) mod normalize_string;
pub(crate) mod pause_gate;
pub(crate) mod ready_state;
pub(crate) mod rgb;
pub(crate) mod shared;
pub(crate) mod shared_fn;
pub(crate) mod slot;
pub(crate) mod span;
pub(crate) mod vec2us;

pub use bit_buffer2::{BitBuffer2, BitIter};
pub use cancel_token::CancelToken;
pub use constants::EPSILON;
pub use debug::is_debug;
pub use file_format::{FileExtensionError, FileFormatResult, SerdeFormat};
pub use float_ext::FloatExt;
pub use fnv::FnvHasher;
pub use introspect::{
    FieldDesc, FieldKind, FieldValue, FloatKind, IntegerKind, IntegerValue, Introspect,
    IntrospectEnum, IntrospectError, IntrospectFloat, IntrospectInteger,
};
pub use key_index_vec::{CompactInsert, KeyIndexKey, KeyIndexVec};
pub use normalize_string::NormalizeString;
pub use pause_gate::{PauseGate, PauseGateCloseGuard};
pub use ready_state::ReadyState;
pub use rgb::Rgb;
pub use serde::Result;
pub use serde::{deserialize, serialize};
pub use shared::Shared;
pub use shared_fn::SharedFn;
pub use slot::{Slot, SlotError};
pub use span::Span;
pub use vec2us::Vec2us;
