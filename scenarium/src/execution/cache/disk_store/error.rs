//! How the store's operations fail, and what the blob format rejects.
//!
//! Everything here is recoverable: a cache is an optimization, so a blob that
//! will not read back, a type with no codec, or a write that did not land
//! degrades into a miss or a skipped store rather than failing the run.

use std::io;
use std::path::PathBuf;
use std::string::FromUtf8Error;

use crate::data::codec::error::CodecError;
use crate::data::data_type::TypeId;
use crate::execution::cache::disk_store::store_outcome::StoreOutcome;

/// A publication that was attempted and did not land, named by the stage that
/// broke so a report says which half of the write failed. Every arm is I/O or
/// an external codec's own failure.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("could not create the cache directory for {}: {source}", path.display())]
    Dir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not begin publishing {}: {source}", path.display())]
    Begin {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not encode the outputs for {}: {source}", path.display())]
    Encode {
        path: PathBuf,
        #[source]
        source: CodecFormatError,
    },
    #[error("could not write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not publish {}: {source}", path.display())]
    Publish {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub(crate) type StoreResult = Result<StoreOutcome, StoreError>;

/// A blob that would not go away. A blob that was never there is not one of
/// these — an eviction wants the file gone, and an absent file already is.
#[derive(Debug, thiserror::Error)]
#[error("failed to remove {}: {source}", path.display())]
pub struct RemovalError {
    pub path: PathBuf,
    #[source]
    pub source: io::Error,
}

/// What the blob format itself rejected — distinct from [`CodecError`], which
/// is whatever an individual codec handed back and which this wraps.
#[derive(Debug, thiserror::Error)]
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
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
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

/// What the blob format's readers and writers return.
pub(crate) type FormatResult<T> = Result<T, CodecFormatError>;
