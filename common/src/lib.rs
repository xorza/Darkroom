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
pub mod serde;
#[cfg(any(test, feature = "test-support"))]
pub mod test_utils;

pub(crate) mod cancel_token;
pub(crate) mod constants;
pub(crate) mod debug;
pub(crate) mod file_format;
pub(crate) mod float_ext;
pub(crate) mod introspect;
pub(crate) mod normalize_string;
pub(crate) mod span;

pub use cancel_token::CancelToken;
pub use constants::EPSILON;
pub use debug::is_debug;
pub use file_format::{FileExtensionError, FileFormatResult, SerdeFormat};
pub use float_ext::FloatExt;
pub use introspect::{
    FieldDesc, FieldKind, FieldValue, FloatKind, IntegerKind, IntegerValue, Introspect,
    IntrospectEnum, IntrospectError, IntrospectFloat, IntrospectInteger,
};
pub use normalize_string::NormalizeString;
pub use serde::{DeserializeError, Lz4SizeError, SerializeError, deserialize, serialize};
pub use span::Span;
