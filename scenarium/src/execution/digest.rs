//! Content digests for node outputs — the cache key for the per-slot RAM cache and
//! the content-addressed disk cache.
//!
//! A node's output is a pure function of its function (identity + version), its
//! resolved input values, the outputs of its upstream producers, and the content of
//! any external files it reads. [`node_digest`] folds exactly that into a 256-bit
//! BLAKE3 digest, reading each `Bind` producer's *already-stamped* `current_digest`
//! (the executor computes digests producer-first, so no recursion or memoization is
//! needed). Equal digests ⇒ identical computation, so the digest is at once the cache
//! key *and* the invalidation signal: change anything upstream and every downstream
//! digest changes — on this machine or any other. See `README.md` Part B.
//!
//! **Trust boundary (what is *not* folded).** The digest is only as honest as these
//! assumptions; violating one is a *false hit* (a stale value served):
//! - **`func_version` is the behavior contract.** Output *types* are folded, but a
//!   lambda whose value logic (or a default for an unbound optional input) changes with
//!   the same signature and no version bump re-uses the old digest. Bump the version.
//! - **`Pure` must be pure.** A `Pure` node that reads hidden state (context resources,
//!   time, RNG) has a stable digest regardless — declare it `Impure` (no digest, never
//!   cached).
//! - **`FsPath` identity is `(len, mtime)`** — a file's own, or a directory's
//!   entries' ([`hash_fs_path_identity`]), so a folder-reading node can be `Pure` and
//!   still re-key when its contents change. A same-size edit within mtime granularity
//!   can slip through; a full content hash is the opt-in resolver.
//! - **Custom-value blob format** is the codec's responsibility, not the digest's — see
//!   `CustomValueCodec::decode`; a breaking codec change needs a `DOMAIN` bump.

use blake3::Hasher;

use crate::data::{DataType, StaticValue};
use crate::execution::cache::Cache;
use crate::execution::program::{ExecutionBinding, ExecutionProgram, NodeIdx};
use crate::func_lambda::PreCheckDigest;
use crate::function::FuncBehavior;

/// Domain separator mixed into every node digest. Bump the suffix to invalidate
/// every cached digest when the hashing scheme itself changes.
const DOMAIN: &[u8] = b"scenarium-cache-v1";

/// 256-bit content digest. Cross-machine stable for a given binary: equal
/// digests mean the same func+version, params, upstream outputs, and file inputs.
/// A newtype, not a bare `[u8; 32]`, so an arbitrary byte array can't silently pose
/// as a digest where one is expected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Digest(pub(crate) [u8; 32]);

impl Digest {
    /// A content digest of arbitrary bytes — the public constructor a func's
    /// [`PreCheck`](crate::func_lambda::PreCheck) uses to turn its input fingerprint
    /// into a digest. The inner bytes stay `pub(crate)`, so a digest can only be
    /// *made* by hashing, never forged from an arbitrary array.
    pub fn hash(bytes: &[u8]) -> Digest {
        Digest(blake3::hash(bytes).into())
    }

    /// Start a [`DigestHasher`] — the fluent builder for combining several values into one
    /// digest (a [`PreCheck`](crate::func_lambda::PreCheck) fingerprinting its inputs, or
    /// the framework's own structural fold).
    pub fn hasher() -> DigestHasher {
        DigestHasher::new()
    }

    /// Fingerprint an `FsPath`'s external identity — a file's `(len, mtime)` or a
    /// directory's sorted entries — the *same* content fold the framework's structural
    /// digest uses for an `FsPath` input ([`hash_fs_path_identity`]). Exposed so a
    /// [`PreCheck`](crate::func_lambda::PreCheck) that reads files can key on their content
    /// without re-implementing the directory walk (or fold it into a multi-field digest via
    /// [`DigestHasher::write_fs_path`]).
    pub fn fs_path(path: &str) -> Digest {
        let mut hasher = DigestHasher::new();
        hasher.write_fs_path(path);
        hasher.finish()
    }

    /// The raw 32 bytes, for folding this digest into another. Read-only — a `Digest` still
    /// can't be *forged* from arbitrary bytes, only made by hashing.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// A fixed-size value that folds into a [`DigestHasher`] as its **little-endian** bytes, so
/// a digest is stable across architectures. Implemented for the primitive number types plus
/// `f32`/`f64` (by bit pattern) and `bool`. `usize`/`isize` are deliberately *not* included
/// — their width is platform-dependent; cast to a fixed width (`x as u64`) first.
pub trait DigestPod {
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
/// digest-friendly writers. Both the framework's structural fold and a
/// [`PreCheck`](crate::func_lambda::PreCheck)'s own digest computation build through it, so
/// the two encode values identically. Deterministic and cross-architecture stable: PODs
/// fold little-endian ([`DigestPod`]), and variable-length data is length-prefixed
/// ([`write_str`](Self::write_str)) so `"ab"+"c"` can't collide with `"a"+"bc"`.
#[derive(Clone, Debug)]
pub struct DigestHasher(Hasher);

impl DigestHasher {
    pub fn new() -> Self {
        DigestHasher(Hasher::new())
    }

    /// Fold raw bytes verbatim (no length prefix) — for fixed-size data: a discriminant
    /// tag, a domain separator, an already-fixed-width field.
    pub fn write_bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.0.update(bytes);
        self
    }

    /// Fold a fixed-size plain-old-data value ([`DigestPod`]) as its little-endian bytes.
    pub fn write_pod<T: DigestPod>(&mut self, value: T) -> &mut Self {
        value.write_le(self);
        self
    }

    /// Fold a length-prefixed byte string (a `u64` length then the bytes), so
    /// concatenations of variable-length data can't collide.
    pub fn write_len_prefixed(&mut self, bytes: &[u8]) -> &mut Self {
        self.write_pod(bytes.len() as u64).write_bytes(bytes)
    }

    /// Fold a length-prefixed string.
    pub fn write_str(&mut self, s: &str) -> &mut Self {
        self.write_len_prefixed(s.as_bytes())
    }

    /// Fold another digest (its fixed 32 bytes).
    pub fn write_digest(&mut self, digest: &Digest) -> &mut Self {
        self.write_bytes(&digest.0)
    }

    /// Fold an `FsPath`'s external identity — a file's `(len, mtime)` or a directory's
    /// sorted entries (see [`Digest::fs_path`]).
    pub fn write_fs_path(&mut self, path: &str) -> &mut Self {
        hash_fs_path_identity(self, path);
        self
    }

    /// Finalize into a [`Digest`].
    pub fn finish(&self) -> Digest {
        Digest(self.0.finalize().into())
    }
}

impl Default for DigestHasher {
    fn default() -> Self {
        Self::new()
    }
}

/// Identity of an external file an `FsPath` input points at, folded into the
/// digest so the same path holding different bytes invalidates the cache. Uses
/// `(len, mtime)` — cheap; a same-size in-place edit within mtime granularity can
/// slip (a full content hash would be the opt-in resolver).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FileId {
    pub(crate) len: u64,
    pub(crate) mtime_ns: u128,
}

/// `(len, mtime)` from an already-resolved [`std::fs::Metadata`] — shared by a file
/// input and each entry of a directory input, so a directory walk doesn't
/// re-`metadata` what `read_dir` already stat'd.
fn file_id_from_meta(meta: &std::fs::Metadata) -> FileId {
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    FileId {
        len: meta.len(),
        mtime_ns,
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

/// Fold the external identity an `FsPath` const points at — a file's `(len, mtime)` or
/// a directory's entry fingerprint ([`hash_fs_path_identity`]) — so a folder-reading
/// node re-keys on its contents. A no-op for any non-`FsPath` value. Kept separate from
/// [`hash_static`] (which folds only the const string, no I/O). A **pre-check** node never
/// reaches here — it replaces the whole structural fold and does its own (possibly finer)
/// file fingerprinting via [`Digest::fs_path`], so it can ignore an irrelevant `.txt`.
fn hash_fs_content(hasher: &mut DigestHasher, value: &StaticValue) {
    if let StaticValue::FsPath(path) = value {
        hash_fs_path_identity(hasher, path);
    }
}

/// Fold a declared port type into `hasher`: a discriminant tag, plus the nominal
/// type id for `Custom`/`Enum` (so two distinct custom types don't collide). The
/// `FsPath` config is identity-irrelevant to the cached bytes, so only the tag is
/// hashed.
fn hash_data_type(hasher: &mut DigestHasher, ty: &DataType) {
    let tag: u8 = match ty {
        DataType::Null => 0,
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

/// A node's **content digest** — the one content key it's cached under, folding its
/// identity (func id/version + output types) plus either its structural inputs or a
/// pre-check's computed digest. The single digest the whole cache keys on: RAM reuse
/// ([`Cache::is_resident_hit`]), disk load/store, and downstream folding all read the
/// node's stamped `current_digest`.
///
/// Computed by the executor as it reaches each node — producer-first (topological). The
/// pre-check (see [`PreCheckDigest`]) decides how inputs fold:
/// - [`None`](PreCheckDigest::None) — no pre-check: fold every input structurally (a
///   `Const`'s value + `FsPath` directory content, or a `Bind` producer's *already-stamped*
///   `current_digest`, whose `None` taints this node to `None`). `None` for an `Impure`
///   node, so it never caches and always runs.
/// - [`Computed`](PreCheckDigest::Computed) — the pre-check owns the whole input
///   contribution: skip the structural fold and mix its digest in instead, so an upstream
///   change that leaves the effective inputs the same doesn't re-key.
/// - [`Uncacheable`](PreCheckDigest::Uncacheable) — the pre-check declined ⇒ `None`.
pub(crate) fn node_digest(
    program: &ExecutionProgram,
    idx: NodeIdx,
    cache: &Cache,
    pre_check: PreCheckDigest,
) -> Option<Digest> {
    let e_node = &program.e_nodes[idx];

    // The pre-check declined this run (e.g. no valid key) — not cacheable.
    if matches!(pre_check, PreCheckDigest::Uncacheable) {
        return None;
    }
    // Only a `Pure` node — or one a pre-check computes a digest for — is content-cacheable;
    // an `Impure` node varies per run, so it has no digest and always recomputes.
    if e_node.behavior != FuncBehavior::Pure && matches!(pre_check, PreCheckDigest::None) {
        return None;
    }

    let mut hasher = DigestHasher::new();
    hasher
        .write_bytes(DOMAIN)
        .write_pod(e_node.func_id.as_u128())
        .write_pod(e_node.func_version);

    let out_types = program.node_output_types(e_node);
    hasher.write_pod(out_types.len() as u64);
    for ty in out_types {
        hash_data_type(&mut hasher, ty);
    }

    if let PreCheckDigest::Computed(digest) = pre_check {
        // The pre-check owns the entire input contribution — no structural fold.
        hasher.write_bytes(&[3]).write_digest(&digest);
    } else {
        for pool_idx in e_node.inputs.range() {
            match &program.inputs[pool_idx].binding {
                ExecutionBinding::None => {
                    hasher.write_bytes(&[0]);
                }
                ExecutionBinding::Const(value) => {
                    hasher.write_bytes(&[1]);
                    hash_static(&mut hasher, value);
                    hash_fs_content(&mut hasher, value);
                }
                ExecutionBinding::Bind(addr) => {
                    // The producer was visited first (topological execute order), so its
                    // `current_digest` is set; a `None` taints this node.
                    let node = cache.slots[addr.target_idx].current_digest?;
                    hasher
                        .write_bytes(&[2])
                        .write_digest(&port_digest_of(node, addr.port_idx));
                }
            }
        }
    }
    Some(hasher.finish())
}

/// Fold an `FsPath` input's external identity into `hasher`: for a regular file its
/// `(len, mtime)`; for a **directory**, a fingerprint of its immediate entries (each
/// entry's name + `(len, mtime)`, sorted for determinism) — so adding, removing, or
/// editing a contained file re-keys the node's digest. This is what lets a node that
/// reads a folder (calibration frames, a light-frame set) be `Pure` and cache
/// correctly: the folder's contents *are* part of the cache key. A path that can't be
/// stat'd folds a distinct "missing" marker. Same `(len, mtime)` trust boundary as the
/// file case, and non-recursive — enough for the flat frame folders these inputs point
/// at; a nested layout only tracks its subdirectories' own mtimes.
fn hash_fs_path_identity(hasher: &mut DigestHasher, path: &str) {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => {
            hasher.write_bytes(&[1]);
            hash_dir_entries(hasher, path);
        }
        Ok(meta) => {
            let id = file_id_from_meta(&meta);
            hasher
                .write_bytes(&[0])
                .write_pod(id.len)
                .write_pod(id.mtime_ns);
        }
        // Missing/unreadable — distinct from both a file and a dir. The path string
        // is folded by the caller, so distinct missing paths stay distinct.
        Err(_) => {
            hasher.write_bytes(&[2]);
        }
    }
}

/// Fold a directory's immediate entries into `hasher` in a deterministic order:
/// entry count, then each entry's name and `(len, mtime)`, sorted by name
/// (`read_dir` order isn't stable across platforms). An unreadable directory folds
/// only its (zero) count.
fn hash_dir_entries(hasher: &mut DigestHasher, dir: &str) {
    let Ok(read) = std::fs::read_dir(dir) else {
        hasher.write_pod(0u64);
        return;
    };
    let mut entries: Vec<(String, FileId)> = read
        .flatten()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let id = entry
                .metadata()
                .map(|m| file_id_from_meta(&m))
                .unwrap_or_default();
            (name, id)
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    hasher.write_pod(entries.len() as u64);
    for (name, id) in &entries {
        hasher
            .write_str(name)
            .write_pod(id.len)
            .write_pod(id.mtime_ns);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::StaticValue;
    use crate::execution::program::{ExecutionInput, ExecutionNode, ExecutionPortAddress};
    use crate::function::FuncId;
    use crate::graph::NodeId;
    use common::Span;

    /// A folder-input node stays `Pure` and cacheable, but its digest tracks the
    /// folder's *contents*: `hash_fs_path_identity` re-keys when a contained file is
    /// added, edited, or removed, and folds identically for an unchanged directory —
    /// closing the gap a bare directory mtime (add/remove only) leaves.
    #[test]
    fn dir_fingerprint_tracks_entry_changes() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "scenarium-digest-dirfp-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();

        let fingerprint = Digest::fs_path;

        std::fs::write(dir.join("a.fits"), b"one").unwrap();
        let base = fingerprint(&path);
        assert_eq!(
            fingerprint(&path),
            base,
            "an unchanged directory folds identically"
        );

        std::fs::write(dir.join("b.fits"), b"two").unwrap();
        let after_add = fingerprint(&path);
        assert_ne!(
            after_add, base,
            "adding a file re-keys the directory fingerprint"
        );

        // In-place edit changing length — the gap a bare directory mtime would miss.
        std::fs::write(dir.join("a.fits"), b"one-plus-more").unwrap();
        let after_edit = fingerprint(&path);
        assert_ne!(after_edit, after_add, "editing a contained file re-keys it");

        std::fs::remove_file(dir.join("b.fits")).unwrap();
        assert_ne!(fingerprint(&path), after_edit, "removing a file re-keys it");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Minimal hand-built `ExecutionProgram` for digest tests. Node ids are
    /// `from_u128(idx + 1)`; `bind`'s target id must match that scheme. Output types
    /// go straight into `program.output_types` — each output defaults to `Int`,
    /// overridable via [`Prog::add_typed`] to exercise the output-signature folding.
    #[derive(Debug, Default)]
    struct Prog {
        program: ExecutionProgram,
    }

    impl Prog {
        /// Add a `Pure` (content-cacheable) node; outputs default to `Int`.
        fn add(
            &mut self,
            func: u128,
            version: u64,
            outputs: u32,
            bindings: &[ExecutionBinding],
        ) -> usize {
            self.add_with(
                FuncBehavior::Pure,
                func,
                version,
                &vec![DataType::Int; outputs as usize],
                bindings,
            )
        }

        /// Add a `Pure` node with explicit output types (the digest folds them).
        fn add_typed(
            &mut self,
            func: u128,
            types: &[DataType],
            bindings: &[ExecutionBinding],
        ) -> usize {
            self.add_with(FuncBehavior::Pure, func, 0, types, bindings)
        }

        /// Add an `Impure` node — its `node_digest` is always `None`.
        fn add_impure(&mut self, func: u128, outputs: u32, bindings: &[ExecutionBinding]) -> usize {
            self.add_with(
                FuncBehavior::Impure,
                func,
                0,
                &vec![DataType::Int; outputs as usize],
                bindings,
            )
        }

        fn add_with(
            &mut self,
            behavior: FuncBehavior,
            func: u128,
            version: u64,
            types: &[DataType],
            bindings: &[ExecutionBinding],
        ) -> usize {
            let inputs_start = self.program.inputs.len() as u32;
            for binding in bindings {
                self.program.inputs.push(ExecutionInput {
                    required: false,
                    binding: binding.clone(),
                });
            }
            let idx = self.program.e_nodes.len();
            let outputs_start = self.program.n_outputs as u32;
            self.program.n_outputs += types.len();
            self.program.output_types.extend_from_slice(types);
            self.program.e_nodes.add(ExecutionNode {
                id: NodeId::from_u128(idx as u128 + 1),
                inited: true,
                behavior,
                func_id: FuncId::from_u128(func),
                func_version: version,
                inputs: Span::new(inputs_start, bindings.len() as u32),
                outputs: Span::new(outputs_start, types.len() as u32),
                ..Default::default()
            });
            idx
        }
    }

    fn bind(idx: usize, port: usize) -> ExecutionBinding {
        ExecutionBinding::Bind(ExecutionPortAddress {
            target_idx: idx.into(),
            port_idx: port,
        })
    }

    fn konst(value: StaticValue) -> ExecutionBinding {
        ExecutionBinding::Const(value)
    }

    /// Fold node digests into a fresh cache the way the executor does — producer-first
    /// in `e_node` order (the test `Prog`s are built that way), each node reading its
    /// producers' just-stamped `current_digest` — stopping after `through`. Re-stats any
    /// `FsPath` const each call. Returns the cache, holding every computed digest.
    fn digested_cache(program: &ExecutionProgram, through: NodeIdx) -> Cache {
        let mut cache = Cache::default();
        cache.reconcile(&program.e_nodes);
        for idx in program.node_indices().take(through.idx() + 1) {
            let d = node_digest(program, idx, &cache, PreCheckDigest::None);
            cache.slots[idx].current_digest = d;
        }
        cache
    }

    /// One node's content digest, computing only the producer-first prefix it needs.
    fn digest_at(program: &ExecutionProgram, idx: NodeIdx) -> Option<Digest> {
        digested_cache(program, idx).slots[idx].current_digest
    }

    /// Every node's content digest, indexed by position.
    fn digests(prog: &Prog) -> Vec<Option<Digest>> {
        let last = NodeIdx::from(prog.program.e_nodes.len().saturating_sub(1));
        let cache = digested_cache(&prog.program, last);
        prog.program
            .node_indices()
            .map(|i| cache.slots[i].current_digest)
            .collect()
    }

    #[test]
    fn deterministic_and_per_function_distinct() {
        // A → B (B binds A.0), plus an independent C with a const input.
        let mut p = Prog::default();
        p.add(10, 0, 1, &[]); // A
        p.add(20, 0, 1, &[bind(0, 0)]); // B
        p.add(30, 0, 1, &[konst(StaticValue::Int(5))]); // C

        let first = digests(&p);
        let second = digests(&p); // same engine inputs → identical digests
        assert_eq!(first, second, "digest must be deterministic");

        // Distinct functions ⇒ distinct digests (no accidental collisions).
        assert_ne!(first[0], first[1]);
        assert_ne!(first[1], first[2]);
        assert_ne!(first[0], first[2]);
    }

    #[test]
    fn const_change_propagates_downstream_only() {
        let build = |a_const: i64| {
            let mut p = Prog::default();
            p.add(10, 0, 1, &[konst(StaticValue::Int(a_const))]); // A
            p.add(20, 0, 1, &[bind(0, 0)]); // B binds A
            p.add(30, 0, 1, &[konst(StaticValue::Int(9))]); // C, independent
            p
        };
        let base = digests(&build(1));
        let changed = digests(&build(2));

        assert_ne!(base[0], changed[0], "A's own digest tracks its const");
        assert_ne!(base[1], changed[1], "B downstream of A must change too");
        assert_eq!(base[2], changed[2], "independent C is unaffected");
    }

    #[test]
    fn version_bump_invalidates_self_and_downstream_but_not_upstream() {
        let build = |a_ver: u64, b_ver: u64| {
            let mut p = Prog::default();
            p.add(10, a_ver, 1, &[]); // A
            p.add(20, b_ver, 1, &[bind(0, 0)]); // B binds A
            p
        };
        let base = digests(&build(0, 0));
        let bump_b = digests(&build(0, 1));
        let bump_a = digests(&build(1, 0));

        // Bumping B (downstream) changes only B.
        assert_eq!(base[0], bump_b[0]);
        assert_ne!(base[1], bump_b[1]);

        // Bumping A (upstream) changes A and ripples into B.
        assert_ne!(base[0], bump_a[0]);
        assert_ne!(base[1], bump_a[1]);
    }

    #[test]
    fn structurally_identical_nodes_share_digest() {
        // Two nodes, same func+version, same (input-identical) bindings ⇒ equal
        // node digests — the property that lets the store dedup repeated work.
        let mut p = Prog::default();
        p.add(10, 2, 1, &[konst(StaticValue::Int(7))]);
        p.add(10, 2, 1, &[konst(StaticValue::Int(7))]);
        let d = digests(&p);
        assert_eq!(d[0], d[1]);

        // Differ in func, version, or const ⇒ digests diverge.
        let mut q = Prog::default();
        q.add(10, 2, 1, &[konst(StaticValue::Int(7))]);
        q.add(11, 2, 1, &[konst(StaticValue::Int(7))]); // different func
        q.add(10, 3, 1, &[konst(StaticValue::Int(7))]); // different version
        q.add(10, 2, 1, &[konst(StaticValue::Int(8))]); // different const
        let dq = digests(&q);
        assert_ne!(dq[0], dq[1]);
        assert_ne!(dq[0], dq[2]);
        assert_ne!(dq[0], dq[3]);
    }

    #[test]
    fn fs_path_folds_file_identity_and_path() {
        // An `FsPath` const folds its resolved file identity (len, mtime — see
        // `fs_file_id`) on top of the path string, so a file change re-keys: this is
        // what stops machine B serving A's result for B's files. The resolver is the
        // real filesystem, so exercise it with a temp file. `digest_at` re-stats it on
        // each call (a fresh engine).
        let file = std::env::temp_dir().join("scenarium_digest_fs_path_test.bin");
        let path = file.to_string_lossy().into_owned();
        let prog_for = |path: &str| {
            let mut p = Prog::default();
            p.add(10, 0, 1, &[konst(StaticValue::FsPath(path.into()))]);
            p
        };

        let p = prog_for(&path);
        std::fs::write(&file, b"x").unwrap(); // len 1
        let d_len1 = digest_at(&p.program, NodeIdx(0));
        std::fs::write(&file, b"xyz").unwrap(); // len 3 — file identity changed
        let d_len3 = digest_at(&p.program, NodeIdx(0));
        assert_ne!(
            d_len1, d_len3,
            "a file content change must re-key the digest"
        );

        // A present file and a missing one are distinct identities.
        std::fs::remove_file(&file).unwrap();
        let d_missing = digest_at(&p.program, NodeIdx(0));
        assert_ne!(d_len3, d_missing, "file presence must matter");

        // The path string itself is folded, independent of file identity (both missing).
        let d_other = digest_at(
            &prog_for("definitely-missing-elsewhere").program,
            NodeIdx(0),
        );
        assert_ne!(d_missing, d_other, "different path ⇒ different digest");
    }

    #[test]
    fn output_ports_are_disambiguated() {
        // One producer with two outputs; consumers on different ports differ.
        let mut p = Prog::default();
        p.add(10, 0, 2, &[]); // A, two outputs
        p.add(20, 0, 1, &[bind(0, 0)]); // B binds A.0
        p.add(20, 0, 1, &[bind(0, 1)]); // C binds A.1 (same func as B)

        let a = digest_at(&p.program, NodeIdx(0)).unwrap();
        assert_ne!(
            port_digest_of(a, 0),
            port_digest_of(a, 1),
            "ports of one node must hash apart"
        );
        let d = digests(&p);
        assert_ne!(d[1], d[2], "consumers reading different ports must differ");
    }

    /// The output signature is part of the key: a flipped type, an added port, or a
    /// distinct custom type re-keys the node, and a producer's change propagates to
    /// its consumers — so a redefined func can never serve a stale blob of the wrong
    /// type without a version bump.
    #[test]
    fn output_signature_folds_into_digest_and_propagates() {
        use crate::data::TypeId;

        // A flipped output type re-keys.
        let mut flip = Prog::default();
        flip.add_typed(10, &[DataType::Int], &[]);
        flip.add_typed(10, &[DataType::Float], &[]);
        let d = digests(&flip);
        assert_ne!(d[0], d[1], "a flipped output type re-keys the node");

        // An added output port re-keys (arity is folded).
        let mut arity = Prog::default();
        arity.add_typed(10, &[DataType::Int], &[]);
        arity.add_typed(10, &[DataType::Int, DataType::Int], &[]);
        let d = digests(&arity);
        assert_ne!(d[0], d[1], "an added output port re-keys the node");

        // Distinct custom types fold their type id — no collision.
        let mut custom = Prog::default();
        custom.add_typed(10, &[DataType::Custom(TypeId::from_u128(1))], &[]);
        custom.add_typed(10, &[DataType::Custom(TypeId::from_u128(2))], &[]);
        let d = digests(&custom);
        assert_ne!(d[0], d[1], "distinct custom output types re-key");

        // A producer's output-type change propagates to its consumer downstream.
        let build = |producer: DataType| {
            let mut g = Prog::default();
            g.add_typed(10, &[producer], &[]); // 0: producer
            g.add_typed(20, &[DataType::Int], &[bind(0, 0)]); // 1: consumes 0.0
            digests(&g)
        };
        let base = build(DataType::Int);
        let changed = build(DataType::Float);
        assert_ne!(
            base[0], changed[0],
            "producer's digest tracks its output type"
        );
        assert_ne!(
            base[1], changed[1],
            "and the change propagates to the consumer"
        );
    }

    #[test]
    fn cycle_yields_none() {
        // A binds B.0, B binds A.0 — a malformed program (the planner rejects it
        // separately); the digest pass must break the recursion, not loop.
        let mut p = Prog::default();
        p.add(10, 0, 1, &[bind(1, 0)]); // A binds B (idx 1)
        p.add(20, 0, 1, &[bind(0, 0)]); // B binds A (idx 0)
        assert_eq!(digest_at(&p.program, NodeIdx(0)), None);
    }

    #[test]
    fn impure_node_and_its_dependents_are_none() {
        // src (impure) → mid (pure) → sink (pure). The impure source taints the
        // whole downstream chain; an independent pure node stays `Some`.
        let mut p = Prog::default();
        p.add_impure(10, 1, &[]); // 0: impure source
        p.add(20, 0, 1, &[bind(0, 0)]); // 1: pure, binds impure
        p.add(30, 0, 1, &[bind(1, 0)]); // 2: pure, binds tainted
        p.add(40, 0, 1, &[konst(StaticValue::Int(5))]); // 3: independent pure

        let d = digests(&p);
        assert_eq!(d[0], None, "impure node ⇒ None");
        assert_eq!(d[1], None, "pure node under impure ⇒ None");
        assert_eq!(d[2], None, "taint flows the whole way up");
        assert!(d[3].is_some(), "independent pure node is unaffected");
    }

    /// A `Computed` pre-check owns the entire input contribution: the structural input
    /// fold is skipped, so the node is keyed on the pre-check digest alone — unaffected by
    /// *which* producer feeds it, and **not** tainted by an impure one (unlike the
    /// structural fold, whose `?` short-circuits on a `None` producer digest). A declined
    /// pre-check (`Uncacheable`) has no digest.
    #[test]
    fn computed_pre_check_owns_inputs_and_ignores_taint() {
        let probe = Digest::hash(b"probe");

        // Consumer (func 20) binding a PURE producer.
        let mut pure = Prog::default();
        pure.add(10, 0, 1, &[]); // 0: pure producer
        pure.add(20, 0, 1, &[bind(0, 0)]); // 1: consumer binds it
        let pure_cache = digested_cache(&pure.program, NodeIdx(1));

        // Same consumer (func 20) binding an IMPURE producer — digest `None`, which would
        // taint the consumer to `None` under the structural fold.
        let mut impure = Prog::default();
        impure.add_impure(10, 1, &[]); // 0: impure producer
        impure.add(20, 0, 1, &[bind(0, 0)]); // 1: consumer binds it
        let impure_cache = digested_cache(&impure.program, NodeIdx(1));

        // Sanity: under the *structural* fold (no pre-check), the impure input taints the
        // consumer to None.
        assert!(
            node_digest(
                &impure.program,
                NodeIdx(1),
                &impure_cache,
                PreCheckDigest::None
            )
            .is_none(),
            "structural fold: an impure producer taints the consumer to None"
        );

        // Computed: keyed on the probe alone — Some in both cases, and IDENTICAL despite
        // one producer being pure and the other impure.
        let c_pure = node_digest(
            &pure.program,
            NodeIdx(1),
            &pure_cache,
            PreCheckDigest::Computed(probe),
        );
        let c_impure = node_digest(
            &impure.program,
            NodeIdx(1),
            &impure_cache,
            PreCheckDigest::Computed(probe),
        );
        assert!(
            c_impure.is_some(),
            "a Computed pre-check is not tainted by an impure input"
        );
        assert_eq!(
            c_pure, c_impure,
            "Computed ignores which producer feeds the input"
        );

        // It also ignores the structural inputs entirely, so it differs from the
        // no-pre-check structural digest of the same node.
        let structural = node_digest(&pure.program, NodeIdx(1), &pure_cache, PreCheckDigest::None);
        assert_ne!(
            c_pure, structural,
            "Computed (probe only) ≠ the structural input fold"
        );

        // A different probe re-keys — the Computed digest *is* the key.
        let c_other = node_digest(
            &pure.program,
            NodeIdx(1),
            &pure_cache,
            PreCheckDigest::Computed(Digest::hash(b"other")),
        );
        assert_ne!(c_pure, c_other, "the Computed digest is the key");

        // A declined pre-check ⇒ not cacheable.
        assert_eq!(
            node_digest(
                &pure.program,
                NodeIdx(1),
                &pure_cache,
                PreCheckDigest::Uncacheable
            ),
            None,
            "a declined pre-check has no digest"
        );
    }

    /// The [`DigestHasher`] builder is deterministic, encodes PODs little-endian and
    /// width-typed, length-prefixes strings so concatenations can't collide, and folds a
    /// nested digest as its raw bytes.
    #[test]
    fn digest_hasher_encodes_deterministically_and_without_collisions() {
        let build = || {
            let mut h = DigestHasher::new();
            h.write_bytes(&[9]).write_pod(7u64).write_str("ab");
            h.finish()
        };
        assert_eq!(build(), build(), "same writes ⇒ same digest");

        let hash_with = |f: &dyn Fn(&mut DigestHasher)| {
            let mut h = DigestHasher::new();
            f(&mut h);
            h.finish()
        };

        // Length-prefixed strings: "ab"+"c" can't collide with "a"+"bc".
        assert_ne!(
            hash_with(&|h| {
                h.write_str("ab").write_str("c");
            }),
            hash_with(&|h| {
                h.write_str("a").write_str("bc");
            }),
            "write_str length-prefixes, so concatenations don't collide"
        );

        // write_pod is width-typed and value-sensitive: 1u64 ≠ 1u32 ≠ 2u64.
        let u64_1 = hash_with(&|h| {
            h.write_pod(1u64);
        });
        assert_ne!(
            u64_1,
            hash_with(&|h| {
                h.write_pod(1u32);
            }),
            "different widths encode differently"
        );
        assert_ne!(
            u64_1,
            hash_with(&|h| {
                h.write_pod(2u64);
            }),
            "different values encode differently"
        );

        // bool folds as one byte; a flip re-keys.
        assert_ne!(
            hash_with(&|h| {
                h.write_pod(true);
            }),
            hash_with(&|h| {
                h.write_pod(false);
            }),
            "a bool flip changes the digest"
        );

        // write_digest folds the nested digest's raw 32 bytes — same as write_bytes(as_bytes()).
        let inner = Digest::hash(b"inner");
        assert_eq!(
            hash_with(&|h| {
                h.write_digest(&inner);
            }),
            hash_with(&|h| {
                h.write_bytes(inner.as_bytes());
            }),
            "write_digest folds the digest's raw bytes"
        );
    }
}
