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
use crate::elements::cache_passthrough::file_cache_digest;
use crate::execution::cache::Cache;
use crate::execution::program::{ExecutionBinding, ExecutionProgram, NodeIdx};
use crate::function::FuncBehavior;
use crate::special::SpecialNode;

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
    let mut hasher = Hasher::new();
    hasher.update(&node.0);
    hasher.update(&(port_idx as u64).to_le_bytes());
    Digest(hasher.finalize().into())
}

/// Fold one constant's *own value* into `hasher`: a discriminant tag plus
/// length-prefixed payload (so `"ab"`+`"c"` can't collide with `"a"`+`"bc"`). For an
/// `FsPath` this is the path *string* only — the external file/dir it points at is a
/// separate concern folded by [`hash_fs_content`], so this stays a pure, no-I/O
/// structural fold. A free helper like [`hash_data_type`].
fn hash_static(hasher: &mut Hasher, value: &StaticValue) {
    match value {
        StaticValue::Null => {
            hasher.update(&[0u8]);
        }
        StaticValue::Float(v) => {
            hasher.update(&[1u8]);
            hasher.update(&v.to_bits().to_le_bytes());
        }
        StaticValue::Int(v) => {
            hasher.update(&[2u8]);
            hasher.update(&v.to_le_bytes());
        }
        StaticValue::Bool(v) => {
            hasher.update(&[3u8, *v as u8]);
        }
        StaticValue::String(s) => {
            hasher.update(&[4u8]);
            hasher.update(&(s.len() as u64).to_le_bytes());
            hasher.update(s.as_bytes());
        }
        StaticValue::FsPath(path) => {
            hasher.update(&[5u8]);
            hasher.update(&(path.len() as u64).to_le_bytes());
            hasher.update(path.as_bytes());
        }
        StaticValue::Enum(name) => {
            hasher.update(&[6u8]);
            hasher.update(&(name.len() as u64).to_le_bytes());
            hasher.update(name.as_bytes());
        }
    }
}

/// Fold the external identity an `FsPath` const points at — a file's `(len, mtime)` or
/// a directory's entry fingerprint ([`hash_fs_path_identity`]) — so a folder-reading
/// node re-keys on its contents. A no-op for any non-`FsPath` value. Kept separate from
/// [`hash_static`] (which folds only the const itself) because a **pre-check** node
/// fingerprints just the files that matter and so *skips* this: folding the whole
/// directory would re-key it on an irrelevant `.txt`.
fn hash_fs_content(hasher: &mut Hasher, value: &StaticValue) {
    if let StaticValue::FsPath(path) = value {
        hash_fs_path_identity(hasher, path);
    }
}

/// Fold a declared port type into `hasher`: a discriminant tag, plus the nominal
/// type id for `Custom`/`Enum` (so two distinct custom types don't collide). The
/// `FsPath` config is identity-irrelevant to the cached bytes, so only the tag is
/// hashed.
fn hash_data_type(hasher: &mut Hasher, ty: &DataType) {
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
    hasher.update(&[tag]);
    if let DataType::Custom(type_id) | DataType::Enum(type_id) = ty {
        hasher.update(&type_id.as_u128().to_le_bytes());
    }
}

/// A node's **content digest** — the one content key it's cached under, folding its
/// identity (func id/version + output types), each input (a `Const`'s value, an unbound
/// marker, or a `Bind` producer's own content digest), and a `pre_check` fingerprint of
/// the external content the func read. The single digest the whole cache keys on: RAM
/// reuse ([`Cache::is_resident_hit`]), disk load/store, and downstream folding all read
/// the node's stamped `current_digest`.
///
/// Computed by the executor as it reaches each node — producer-first (topological), so a
/// `Bind` reads the producer's *already-stamped* `current_digest` (`None` there taints
/// this node to `None`). `None` for a non-reproducible node — `Impure` with no pre-check
/// — so it never caches and always runs. Two special cases:
/// a **`CachePassthrough`** node is keyed on its path input alone (not its cone); a
/// **pre-check** node folds its `Const` `FsPath`s *shallowly* (path string only — its
/// pre-check owns their content, so an irrelevant `.txt` doesn't re-key it), while a
/// pre-check-less node folds the directory walk.
pub(crate) fn node_digest(
    program: &ExecutionProgram,
    idx: NodeIdx,
    cache: &Cache,
    pre_check: Option<Digest>,
) -> Option<Digest> {
    let e_node = &program.e_nodes[idx];

    // A file-cache node is keyed on its path input alone — input 0 can be
    // impure/expensive and it still presents a digest (the reproducibility boundary).
    if matches!(e_node.special, Some(SpecialNode::CachePassthrough { .. })) {
        return file_cache_digest(program.node_inputs(e_node));
    }
    // Only a `Pure` node — or one vouched by a pre-check — is content-cacheable;
    // an `Impure` node varies per run, so it has no digest and always recomputes.
    if e_node.behavior != FuncBehavior::Pure && pre_check.is_none() {
        return None;
    }

    let mut hasher = Hasher::new();
    hasher.update(DOMAIN);
    hasher.update(&e_node.func_id.as_u128().to_le_bytes());
    hasher.update(&e_node.func_version.to_le_bytes());

    let out_types = program.node_output_types(e_node);
    hasher.update(&(out_types.len() as u64).to_le_bytes());
    for ty in out_types {
        hash_data_type(&mut hasher, ty);
    }

    for pool_idx in e_node.inputs.range() {
        match &program.inputs[pool_idx].binding {
            ExecutionBinding::None => {
                hasher.update(&[0u8]);
            }
            ExecutionBinding::Const(value) => {
                hasher.update(&[1u8]);
                hash_static(&mut hasher, value);
                // A pre-check node fingerprints its own files, so it skips the folder
                // walk (which would re-key on an irrelevant `.txt`); a pre-check-less
                // deferred node has no other way to track them, so it folds them.
                if pre_check.is_none() {
                    hash_fs_content(&mut hasher, value);
                }
            }
            ExecutionBinding::Bind(addr) => {
                // The producer was visited first (topological execute order) or was
                // plan-keyed, so its `current_digest` is set; a `None` taints this node.
                let node = cache.slots[addr.target_idx].current_digest?;
                hasher.update(&[2u8]);
                hasher.update(&port_digest_of(node, addr.port_idx).0);
            }
        }
    }
    // The external fingerprint the framework can't see (which files actually matter).
    if let Some(pre_check) = pre_check {
        hasher.update(&[3u8]);
        hasher.update(&pre_check.0);
    }
    Some(Digest(hasher.finalize().into()))
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
fn hash_fs_path_identity(hasher: &mut Hasher, path: &str) {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => {
            hasher.update(&[1u8]);
            hash_dir_entries(hasher, path);
        }
        Ok(meta) => {
            hasher.update(&[0u8]);
            let id = file_id_from_meta(&meta);
            hasher.update(&id.len.to_le_bytes());
            hasher.update(&id.mtime_ns.to_le_bytes());
        }
        // Missing/unreadable — distinct from both a file and a dir. The path string
        // is folded by the caller, so distinct missing paths stay distinct.
        Err(_) => {
            hasher.update(&[2u8]);
        }
    }
}

/// Fold a directory's immediate entries into `hasher` in a deterministic order:
/// entry count, then each entry's name and `(len, mtime)`, sorted by name
/// (`read_dir` order isn't stable across platforms). An unreadable directory folds
/// only its (zero) count.
fn hash_dir_entries(hasher: &mut Hasher, dir: &str) {
    let Ok(read) = std::fs::read_dir(dir) else {
        hasher.update(&0u64.to_le_bytes());
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
    hasher.update(&(entries.len() as u64).to_le_bytes());
    for (name, id) in &entries {
        hasher.update(&(name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update(&id.len.to_le_bytes());
        hasher.update(&id.mtime_ns.to_le_bytes());
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

        let fingerprint = |p: &str| {
            let mut h = Hasher::new();
            hash_fs_path_identity(&mut h, p);
            h.finalize()
        };

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
            let d = node_digest(program, idx, &cache, None);
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
}
