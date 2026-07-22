//! Content digests for node outputs — the validity key for the per-slot RAM cache
//! and the node-keyed disk cache.
//!
//! A node's output is a pure function of its function identity and version, its resolved input
//! values, the outputs of its upstream producers, and the content of
//! any external files it reads. [`node_digest`] folds exactly that into a 256-bit
//! BLAKE3 digest, reading each `Bind` producer's *already-stamped* `current_digest`
//! (the resolver computes digests producer-first, so no recursive digest traversal is
//! needed). External identities come from one memoized per-run
//! [`RunResourceStamps`](crate::execution::resource::RunResourceStamps), keeping this fold
//! I/O-free. Equal digests ⇒ identical computation, so the digest is at once the cache key
//! and the invalidation signal: change anything upstream and every downstream digest
//! changes — on this machine or any other. See `README.md` Part B.
//!
//! **Trust boundary (what is *not* folded).** The digest is only as honest as these
//! assumptions; violating one is a *false hit* (a stale value served):
//! - **`Func::version` is the implementation contract.** Bump it when a lambda can return
//!   different values for the same inputs; leaving it unchanged can reuse an old digest.
//! - **`Pure` must be pure.** A `Pure` node that reads hidden state (context resources,
//!   time, RNG) has a stable digest regardless — declare it `Impure` (no digest, never
//!   cached).
//! - **`FsPath` identity is `(len, mtime)`** — a file's own, or a directory's entries',
//!   prepared by [`RunResourceStamps`](crate::execution::resource::RunResourceStamps), so a
//!   folder-reading node can be `Pure` and still re-key when its contents change. A
//!   same-size edit within mtime granularity
//!   can slip through; a full content hash is the opt-in resolver. The same tier holds
//!   for any registered [`ResourceStamper`](crate::ResourceStamper): a stamp is
//!   cheap referent *metadata*, and stamps are machine-local (mtimes, local versions),
//!   so resource-keyed blobs don't transfer across machines.
//! - **A reference is dereferenced only through an input declared with its resource
//!   type.** Const and Bind-delivered references both fold the referent's identity, but
//!   only where the consumer's input is declared `FsPath` (or a stamper-registered custom
//!   type) — a lambda that reads a file through an `Any`/`String` input keys nothing and
//!   can serve stale content. Declare the type.
//! - **Custom-value blob format** is disk identity, not value identity. Each blob separately
//!   stamps the versions of the codecs its values use; changing one invalidates only relevant
//!   disk blobs without discarding semantically unchanged RAM values or downstream digests.

use blake3::Hasher;

use crate::execution::cache::RuntimeCache;
use crate::execution::identity::{ExecutionNodeId, ExecutionOutputPort};
use crate::execution::program::{ExecutionBinding, ExecutionProgram, InputStamper};
use crate::execution::resource::RunResourceStamps;
use crate::node::definition::FuncBehavior;
use crate::{DataType, StaticValue};

/// Domain separator mixed into every node digest. Bump the suffix to invalidate
/// every cached digest when the hashing scheme itself changes.
const DOMAIN: &[u8] = b"scenarium-cache-v3";

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
pub(crate) trait DigestPod {
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

/// A fluent builder for a [`Digest`] — a thin wrapper over the BLAKE3 hasher with
/// digest-friendly writers, used by the framework's structural fold. Deterministic and
/// cross-architecture stable: PODs fold little-endian
/// ([`DigestPod`]), and variable-length data is length-prefixed
/// ([`write_str`](Self::write_str)) so `"ab"+"c"` can't collide with `"a"+"bc"`.
#[derive(Clone, Debug)]
pub(crate) struct DigestHasher(Hasher);

impl DigestHasher {
    pub(crate) fn new() -> Self {
        DigestHasher(Hasher::new())
    }

    /// Fold raw bytes verbatim (no length prefix) — for fixed-size data: a discriminant
    /// tag, a domain separator, an already-fixed-width field.
    pub(crate) fn write_bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.0.update(bytes);
        self
    }

    /// Fold a fixed-size plain-old-data value ([`DigestPod`]) as its little-endian bytes.
    pub(crate) fn write_pod<T: DigestPod>(&mut self, value: T) -> &mut Self {
        value.write_le(self);
        self
    }

    /// Fold a length-prefixed byte string (a `u64` length then the bytes), so
    /// concatenations of variable-length data can't collide.
    pub(crate) fn write_len_prefixed(&mut self, bytes: &[u8]) -> &mut Self {
        self.write_pod(bytes.len() as u64).write_bytes(bytes)
    }

    /// Fold a length-prefixed string.
    pub(crate) fn write_str(&mut self, s: &str) -> &mut Self {
        self.write_len_prefixed(s.as_bytes())
    }

    /// Fold another digest (its fixed 32 bytes).
    pub(crate) fn write_digest(&mut self, digest: &Digest) -> &mut Self {
        self.write_bytes(&digest.0)
    }

    /// Finalize into a [`Digest`].
    pub(crate) fn finish(&self) -> Digest {
        Digest(self.0.finalize().into())
    }
}

impl Default for DigestHasher {
    fn default() -> Self {
        Self::new()
    }
}

/// One output *port*'s digest from its node's digest — the node digest mixed with the
/// port index, so two consumers reading different ports of one node hash apart. Folded
/// by [`node_digest`] for each `Bind` producer.
fn port_digest_of(node: Digest, port_idx: usize) -> Digest {
    let mut hasher = DigestHasher::new();
    hasher.write_digest(&node).write_pod(port_idx as u64);
    hasher.finish()
}

/// Fold one constant's *own value* into `hasher`: a discriminant tag plus
/// length-prefixed payload (so `"ab"`+`"c"` can't collide with `"a"`+`"bc"`). For an
/// `FsPath` this is the path *string* only — the external file/dir it points at is a
/// separate concern folded by [`hash_fs_content`], so this stays a pure, no-I/O
/// structural fold. A free helper like [`hash_data_type`].
fn hash_static(hasher: &mut DigestHasher, value: &StaticValue) {
    match value {
        StaticValue::Null => {
            hasher.write_bytes(&[0]);
        }
        StaticValue::Float(v) => {
            hasher.write_bytes(&[1]).write_pod(*v);
        }
        StaticValue::Int(v) => {
            hasher.write_bytes(&[2]).write_pod(*v);
        }
        StaticValue::Bool(v) => {
            hasher.write_bytes(&[3]).write_pod(*v);
        }
        StaticValue::String(s) => {
            hasher.write_bytes(&[4]).write_str(s);
        }
        StaticValue::FsPath(path) => {
            hasher.write_bytes(&[5]).write_str(path);
        }
        StaticValue::Enum(name) => {
            hasher.write_bytes(&[6]).write_str(name);
        }
    }
}

/// Fold the prepared external identity an `FsPath` const points at. A no-op for any
/// non-`FsPath` value.
fn hash_fs_content(
    hasher: &mut DigestHasher,
    value: &StaticValue,
    resource_stamps: &RunResourceStamps,
) -> Option<()> {
    if let StaticValue::FsPath(path) = value {
        resource_stamps.hash_fs_path(hasher, path)?;
    }
    Some(())
}

/// Fold a declared port type into `hasher`: a discriminant tag, plus the nominal
/// type id for `Custom`/`Enum` (so two distinct custom types don't collide). The
/// `FsPath` config is identity-irrelevant to the cached bytes, so only the tag is
/// hashed.
fn hash_data_type(hasher: &mut DigestHasher, ty: &DataType) {
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
    hasher.write_bytes(&[tag]);
    if let DataType::Custom(type_id) | DataType::Enum(type_id) = ty {
        hasher.write_pod(type_id.as_u128());
    }
}

/// A node's **content digest** — the one content key it's cached under, folding its identity
/// (func id + version + output types) plus its structural inputs. The single digest the whole
/// cache keys on: RAM reuse ([`RuntimeCache::is_resident_hit`]), disk load/store, and downstream
/// folding all read the node's stamped `current_digest`. Computed producer-first
/// (topological), so a `Bind` producer's `current_digest` is already stamped when read.
///
/// - An **`Impure`** node has no digest (`None`) — it varies per run, so it never caches and
///   always recomputes; a `Bind` producer with a `None` digest taints this node to `None`.
/// - Otherwise fold every input structurally: a `Const`'s value + prepared `FsPath`
///   file/dir content, or a `Bind` producer's stamped `current_digest` — plus, for a
///   resource-typed input, the live identity of the referent behind the *delivered* value
///   ([`hash_bound_resource`]). That last fold needs the producer's value: unreadable ⇒
///   `None`, and the run loop re-stamps such a node at reach time, once its producers settled.
pub(crate) fn node_digest(
    program: &ExecutionProgram,
    e_node_id: ExecutionNodeId,
    cache: &RuntimeCache,
    resource_stamps: &RunResourceStamps,
) -> Option<Digest> {
    let e_node = &program.e_nodes[&e_node_id];

    // Only a `Pure` node is content-cacheable; an `Impure` node varies per run, so it has no
    // digest and always recomputes.
    if e_node.behavior != FuncBehavior::Pure {
        return None;
    }

    let mut hasher = DigestHasher::new();
    hasher
        .write_bytes(DOMAIN)
        .write_pod(e_node.func_id.as_u128())
        .write_pod(e_node.version);

    let out_types = program.node_output_types(e_node);
    hasher.write_pod(out_types.len() as u64);
    for ty in out_types {
        hash_data_type(&mut hasher, ty);
    }

    for pool_idx in e_node.inputs.range() {
        let input = &program.inputs[pool_idx];
        match &input.binding {
            ExecutionBinding::None => {
                hasher.write_bytes(&[0]);
            }
            ExecutionBinding::Const(value) => {
                hasher.write_bytes(&[1]);
                hash_static(&mut hasher, value);
                hash_fs_content(&mut hasher, value, resource_stamps)?;
            }
            ExecutionBinding::Bind(addr) => {
                // The producer was visited first (topological order), so its `current_digest`
                // is set; a `None` taints this node.
                let node = cache.slots[&addr.e_node_id].current_digest?;
                hasher
                    .write_bytes(&[2])
                    .write_digest(&port_digest_of(node, addr.port_idx));
                // A resource-typed input dereferences the delivered reference, so the
                // external state behind the *runtime value* is part of this node's key —
                // the Bind-side counterpart of the `Const` arm's `hash_fs_content`. Needs
                // the producer's value; unreadable (pre-run) ⇒ `None`, re-stamped at reach
                // time by the run loop.
                if let Some(stamper) = &input.stamper {
                    hash_bound_resource(&mut hasher, cache, resource_stamps, addr, stamper)?;
                }
            }
        }
    }
    Some(hasher.finish())
}

/// Fold the referent identity behind a **Bind-delivered** resource input: read the
/// delivered value off the producer's resident slot and fold what the input's
/// [`InputStamper`] derives from it — the built-in prepared `FsPath` file/dir identity,
/// or a registered [`ResourceStamper`](crate::ResourceStamper)'s prepared stamp bytes
/// (length-prefixed, so its internal encoding can't collide across calls) — so a wired
/// reference re-keys its consumer exactly like a const path does. The producer's value
/// must exist first: an unreadable value (producer not resident) is `None`, tainting the
/// node's digest — the pre-run sweep stamps it "uncacheable, must run", and the run loop
/// then *re-stamps* at reach time, when the producers have settled and any disk-backed
/// resource producer was hydrated (`executor.rs`). A value the built-in path stamper
/// can't read as a path (a mis-typed wire) folds a distinct marker instead.
fn hash_bound_resource(
    hasher: &mut DigestHasher,
    cache: &RuntimeCache,
    resource_stamps: &RunResourceStamps,
    addr: &ExecutionOutputPort,
    stamper: &InputStamper,
) -> Option<()> {
    let value = cache.slots[&addr.e_node_id]
        .current_output_values()?
        .get(addr.port_idx)?;
    match stamper {
        InputStamper::FsPath => match value.as_fs_path() {
            Some(path) => {
                hasher.write_bytes(&[3]);
                resource_stamps.hash_fs_path(hasher, path)?;
            }
            None => {
                hasher.write_bytes(&[4]);
            }
        },
        InputStamper::Custom(stamper) => {
            hasher.write_bytes(&[5]);
            resource_stamps.hash_custom(hasher, addr, stamper, value)?;
        }
    }
    Some(())
}

#[cfg(test)]
mod tests;
