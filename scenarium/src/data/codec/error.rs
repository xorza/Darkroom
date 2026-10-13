//! What the disk cache's custom-value codec path rejects.
//!
//! Everything here is recoverable: a blob that will not read back, a type whose
//! codec is not registered in the current library, a codec that failed on the
//! value it was handed. A cache is an optimization, so each of these degrades
//! into a miss or a skipped store rather than failing the run.

use std::error;
use std::io;
use std::result;
use std::string::FromUtf8Error;

use thiserror::Error;

use crate::data::data_type::TypeId;

/// The failure a [`CustomValueCodec`](crate::data::codec::CustomValueCodec)
/// hands back. Codecs are written outside this crate — the value type, its
/// representation, and whatever I/O reaching it takes are all the implementor's
/// — so the error stays theirs to name, and this only asks that it can cross a
/// thread and be reported.
pub type CodecError = Box<dyn error::Error + Send + Sync>;

/// What the blob format itself rejected — distinct from [`CodecError`], which
/// is whatever an individual codec handed back and which this wraps.
#[derive(Debug, Error)]
pub enum CodecFormatError {
    #[error("cache I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("malformed cached output frame: {0}")]
    Frame(#[from] FrameError),
    #[error("encoding a {type_id:?} value failed: {source}")]
    Encode { type_id: TypeId, source: CodecError },
    #[error("no cache codec registered for custom type {0:?}")]
    UnknownType(TypeId),
    #[error("decoding a {type_id:?} value failed: {source}")]
    Decode { type_id: TypeId, source: CodecError },
}

/// How a blob's framing is malformed — the bytes are there, but do not add up
/// to a blob this build wrote.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FrameError {
    #[error("cache header has the wrong magic")]
    WrongMagic,
    #[error("cache header has an unsupported version")]
    UnsupportedVersion,
    #[error("cache header length overflow")]
    HeaderLengthOverflow,
    #[error("cache file length overflow")]
    FileLengthOverflow,
    #[error("cache header and body lengths do not equal the file length")]
    FileLengthMismatch,
    #[error("cache payload lengths overflow u64")]
    PayloadLengthOverflow,
    #[error("cache descriptor lengths do not equal the declared body length")]
    BodyLengthMismatch,
    #[error("cache descriptor reserved bytes are not zero")]
    ReservedBytes,
    #[error("unbound cache descriptor carries metadata")]
    UnboundMetadata,
    #[error("static cache descriptor carries codec metadata")]
    StaticMetadata,
    #[error("cache descriptor has an unknown value tag")]
    UnknownValueTag,
    #[error("cached const value has an unknown variant")]
    UnknownConstVariant,
    #[error("cached boolean is not encoded as zero or one")]
    InvalidBool,
    #[error("cached static payload has length {actual}, expected {expected}")]
    PayloadLength { actual: u64, expected: u64 },
    #[error("cached payload decoder did not consume its complete region")]
    PayloadNotConsumed,
    #[error("cached string length overflow")]
    StringLengthOverflow,
    #[error("cached string does not fit in memory")]
    StringTooLarge,
    #[error("cached text is not valid UTF-8: {0}")]
    InvalidUtf8(#[source] FromUtf8Error),
    #[error("cached path list length overflow")]
    PathListLengthOverflow,
    #[error("cached path list count exceeds its payload")]
    PathCountExceedsPayload,
    #[error("cached path count does not fit in memory")]
    PathCountTooLarge,
    #[error("cached path length exceeds its payload")]
    PathLengthExceedsPayload,
    #[error("cached path does not fit in memory")]
    PathTooLarge,
}

pub(crate) type Result<T> = result::Result<T, CodecFormatError>;
