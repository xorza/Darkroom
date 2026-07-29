//! Content digests for node outputs — the validity key for the per-slot RAM cache
//! and the node-keyed disk cache.
//!
//! A node's output is a pure function of its function identity and version, its resolved input
//! values, the outputs of its upstream producers, and the prepared identities of
//! external resources it reads.
//! [`RuntimeCache::node_digest`](crate::execution::cache::runtime::RuntimeCache::node_digest)
//! folds that into a 256-bit
//! BLAKE3 digest, reading each `Bind` producer's *already-stamped* `current_digest`
//! (the resolver computes digests producer-first, so no recursive digest traversal is
//! needed). This module owns the *encoding* — the hasher, the tags, and the [`DOMAIN`]
//! versioning them; the fold itself is a cache method, since all three things it reads
//! (slots, the path memo, the program) are the cache's. External identities come from that
//! memo, filled once per run by a [`StampJob`](crate::execution::cache::resource::StampJob),
//! keeping this fold I/O-free.
//! Equal digests ⇒ identical computation, so the digest is at once the cache key
//! and the invalidation signal: change anything upstream and every downstream digest
//! changes — on this machine or any other. See `README.md` Part B.
//!
//! **Trust boundary (what is *not* folded).** The digest is only as honest as these
//! assumptions; violating one is a *false hit* (a stale value served):
//! - **`Func::version` is the implementation contract.** Bump it when a lambda can return
//!   different values for the same inputs; leaving it unchanged can reuse an old digest.
//!   A bump also drops the node's persistent `state`/`event_state` at the next install
//!   ([`RuntimeCache::reconcile`](crate::execution::cache::runtime::RuntimeCache::reconcile)),
//!   so a new implementation never inherits its
//!   predecessor's state.
//! - **`Pure` must be pure.** A `Pure` node that reads hidden state (context resources,
//!   time, RNG) has a stable digest regardless — declare it `Impure` (no digest, never
//!   cached).
//! - **`FsPath` identity is `(len, mtime)`** — a file's own, or that of every file
//!   beneath a directory (empty directories are not part of it),
//!   prepared by [`RuntimeCache::prepare`](crate::execution::cache::runtime::RuntimeCache::prepare), so a
//!   folder-reading node can be `Pure` and still re-key when its contents change. A
//!   same-size edit within mtime granularity can slip through; explicit runtime cache
//!   eviction removes the affected node and downstream blobs. The same tier holds
//! - **A reference is dereferenced only through an input declared with its resource
//!   type.** Const and Bind-delivered references both fold the referent's identity, but
//!   only where the consumer's input is declared `FsPath` — a lambda that reads a file
//!   through an `Any`/`String` input keys nothing and can serve stale content. Declare the type.
//! - **Custom-value blob format** is disk identity, not value identity. Each blob separately
//!   stamps the versions of the codecs its values use; changing one invalidates only relevant
//!   disk blobs without discarding semantically unchanged RAM values or downstream digests.

use blake3::Hasher;

use crate::{DataType, StaticValue};

/// Domain separator mixed into every node digest. Bump the suffix to invalidate
/// every cached digest when the hashing scheme itself changes.
pub(super) const DOMAIN: &[u8] = b"scenarium-cache-v4";

/// 256-bit content digest. Cross-machine stable for a given binary: equal
/// digests mean the same function identity and version, params, upstream outputs, and file inputs.
/// A newtype, not a bare `[u8; 32]`, so an arbitrary byte array can't silently pose
/// as a digest where one is expected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Digest(pub(crate) [u8; 32]);

/// A fixed-size value that folds into a [`DigestHasher`] as its **little-endian** bytes, so
/// a digest is stable across architectures. Implemented for the primitive number types plus
/// `f32`/`f64` (by bit pattern) and `bool`. `usize`/`isize` are deliberately *not* included
/// — their width is platform-dependent; cast to a fixed width (`x as u64`) first.
pub(super) trait DigestPod {
    fn write_le(self, hasher: &mut DigestHasher);
}

macro_rules! digest_pod_ints {
    ($($t:ty),*) => {
        $(impl DigestPod for $t {
            fn write_le(self, hasher: &mut DigestHasher) {
                hasher.write_bytes(&self.to_le_bytes());
            }
        })*
    };
}
digest_pod_ints!(u8, u16, u32, u64, u128, i8, i16, i32, i64, i128);

impl DigestPod for f32 {
    fn write_le(self, hasher: &mut DigestHasher) {
        hasher.write_bytes(&self.to_bits().to_le_bytes());
    }
}
impl DigestPod for f64 {
    fn write_le(self, hasher: &mut DigestHasher) {
        hasher.write_bytes(&self.to_bits().to_le_bytes());
    }
}
impl DigestPod for bool {
    fn write_le(self, hasher: &mut DigestHasher) {
        hasher.write_bytes(&[self as u8]);
    }
}

/// One input's discriminant in a node digest.
///
/// The whole space in one place, because it is written from two folds —
/// the per-input match and the bound-path fold beneath it — and a value
/// repeated between them would silently make two different inputs key
/// alike. Written through [`DigestHasher::write_input_tag`], so the byte
/// each name stands for is decided once.
#[derive(Clone, Copy, Debug)]
pub(super) enum InputTag {
    /// Nothing bound.
    Unbound = 0,
    /// An authored constant, its value following.
    Const = 1,
    /// A producer port, its digest and port index following.
    Bind = 2,
    /// A resource input, the referent's identity following.
    BoundPaths = 3,
    /// A resource input handed something that is not a path — the marker
    /// stands alone.
    BoundMistyped = 4,
}

/// A fluent builder for a [`Digest`] — a thin wrapper over the BLAKE3 hasher with
/// digest-friendly writers, used by the framework's structural fold. Deterministic and
/// cross-architecture stable: PODs fold little-endian
/// ([`DigestPod`]), and variable-length data is length-prefixed
/// ([`write_str`](Self::write_str)) so `"ab"+"c"` can't collide with `"a"+"bc"`.
#[derive(Clone, Debug)]
pub(super) struct DigestHasher(Hasher);

impl DigestHasher {
    pub(super) fn new() -> Self {
        DigestHasher(Hasher::new())
    }

    /// Fold raw bytes verbatim (no length prefix) — for fixed-size data: a discriminant
    /// tag, a domain separator, an already-fixed-width field.
    pub(super) fn write_bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.0.update(bytes);
        self
    }

    /// Fold one input's discriminant — the one place an [`InputTag`] name
    /// becomes a byte.
    pub(super) fn write_input_tag(&mut self, tag: InputTag) -> &mut Self {
        self.write_bytes(&[tag as u8])
    }

    /// Fold a fixed-size plain-old-data value ([`DigestPod`]) as its little-endian bytes.
    pub(super) fn write_pod<T: DigestPod>(&mut self, value: T) -> &mut Self {
        value.write_le(self);
        self
    }

    /// Fold a length-prefixed byte string (a `u64` length then the bytes), so
    /// concatenations of variable-length data can't collide.
    pub(super) fn write_len_prefixed(&mut self, bytes: &[u8]) -> &mut Self {
        self.write_pod(bytes.len() as u64).write_bytes(bytes)
    }

    /// Fold a length-prefixed string.
    pub(super) fn write_str(&mut self, s: &str) -> &mut Self {
        self.write_len_prefixed(s.as_bytes())
    }

    /// Fold another digest (its fixed 32 bytes).
    pub(super) fn write_digest(&mut self, digest: &Digest) -> &mut Self {
        self.write_bytes(&digest.0)
    }

    /// Finalize into a [`Digest`].
    pub(super) fn finish(&self) -> Digest {
        Digest(self.0.finalize().into())
    }

    /// Fold one constant's *own value*: a discriminant tag plus a
    /// length-prefixed payload (so `"ab"`+`"c"` can't collide with
    /// `"a"`+`"bc"`).
    ///
    /// Filesystem-path values fold only their authored path string(s) here — the
    /// external files/dirs they point at are a separate concern, folded by the
    /// caller through `RuntimeCache::hash_fs_paths`, so this stays a pure,
    /// no-I/O structural fold.
    pub(super) fn write_static(&mut self, value: &StaticValue) {
        match value {
            StaticValue::Null => {
                self.write_bytes(&[0]);
            }
            StaticValue::Float(v) => {
                self.write_bytes(&[1]).write_pod(*v);
            }
            StaticValue::Int(v) => {
                self.write_bytes(&[2]).write_pod(*v);
            }
            StaticValue::Bool(v) => {
                self.write_bytes(&[3]).write_pod(*v);
            }
            StaticValue::String(s) => {
                self.write_bytes(&[4]).write_str(s);
            }
            StaticValue::FsPath(path) => {
                self.write_bytes(&[5]).write_str(path);
            }
            StaticValue::FsPaths(paths) => {
                self.write_bytes(&[6]).write_pod(paths.len() as u64);
                for path in paths {
                    self.write_str(path);
                }
            }
            StaticValue::Enum(name) => {
                self.write_bytes(&[7]).write_str(name);
            }
        }
    }

    /// Fold a declared port type: a discriminant tag, plus the nominal type id
    /// for `Custom`/`Enum` (so two distinct custom types don't collide). The
    /// `FsPath` config is identity-irrelevant to the cached bytes, so only the
    /// tag is hashed.
    pub(super) fn write_data_type(&mut self, ty: &DataType) {
        let tag: u8 = match ty {
            DataType::Any => 0,
            DataType::Float => 1,
            DataType::Int => 2,
            DataType::Bool => 3,
            DataType::String => 4,
            DataType::FsPath(_) => 5,
            DataType::Custom(_) => 6,
            DataType::Enum(_) => 7,
        };
        self.write_bytes(&[tag]);
        if let DataType::Custom(type_id) | DataType::Enum(type_id) = ty {
            self.write_pod(type_id.as_u128());
        }
    }
}

impl Default for DigestHasher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
