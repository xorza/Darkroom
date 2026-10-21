//! What a custom-value codec hands back when it fails.

use std::error;

/// The failure a [`CustomValueCodec`](crate::data::codec::CustomValueCodec)
/// hands back. Codecs are written outside this crate — the value type, its
/// representation, and whatever I/O reaching it takes are all the implementor's
/// — so the error stays theirs to name, and this only asks that it can cross a
/// thread and be reported.
pub type CodecError = Box<dyn error::Error + Send + Sync>;
