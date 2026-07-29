//! What the disk cache's custom-value codec path rejects.
//!
//! Everything here is recoverable: a blob that will not read back, a type whose
//! codec is not registered in the current library, a codec that failed on the
//! value it was handed. A cache is an optimization, so each of these degrades
//! into a miss or a skipped store rather than failing the run.

use thiserror::Error;

use crate::CodecError;
use crate::data::type_system::TypeId;

#[derive(Debug, Error)]
pub(crate) enum Error {
    #[error("cache I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed cached output frame: {0}")]
    Frame(String),
    #[error("encoding a {type_id:?} value failed: {source}")]
    Encode { type_id: TypeId, source: CodecError },
    #[error("no cache codec registered for custom type {0:?}")]
    UnknownType(TypeId),
    #[error("decoding a {type_id:?} value failed: {source}")]
    Decode { type_id: TypeId, source: CodecError },
}

pub(crate) type Result<T> = std::result::Result<T, Error>;
