//! Content digests for node outputs — the cache key for the per-slot RAM cache and
//! the content-addressed disk cache.
//!
//! A node's output is a pure function of its function (identity + version), its
//! resolved input values, the outputs of its upstream producers, and the content
//! of any external files it reads. [`DigestEngine`] folds exactly that into a
//! 256-bit BLAKE3 digest over the flattened [`ExecutionProgram`], memoized per
//! node. Equal digests ⇒ identical computation, so the digest is at once the
//! cache key *and* the invalidation signal: change anything upstream and every
//! downstream digest changes — on this machine or any other. See
//! `README.md` Part B.
//!
//! **Trust boundary (what is *not* folded).** The digest is only as honest as these
//! assumptions; violating one is a *false hit* (a stale value served):
//! - **`func_version` is the behavior contract.** Output *types* are folded, but a
//!   lambda whose value logic (or a default for an unbound optional input) changes with
//!   the same signature and no version bump re-uses the old digest. Bump the version.
//! - **`Pure` must be pure.** A `Pure` node that reads hidden state (context resources,
//!   time, RNG) has a stable digest regardless — declare it `Impure` (no digest, never
//!   cached).
//! - **`FsPath` identity is `(len, mtime)`** ([`fs_file_id`]) — a same-size edit within
//!   mtime granularity can slip through; a full content hash is the opt-in resolver.
//! - **Custom-value blob format** is the codec's responsibility, not the digest's — see
//!   `CustomValueCodec::decode`; a breaking codec change needs a `DOMAIN` bump.

use blake3::Hasher;

use crate::data::{DataType, StaticValue};
use crate::elements::cache_passthrough::file_cache_digest;
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Digest(pub(crate) [u8; 32]);

/// One node's memoized digest result. A node has a digest iff its whole cone is
/// reproducible; an impure node (or any impure ancestor) is `NotCacheable`, which
/// is at once "never RAM-cache" and "never disk-cache" — `Some`/`None` carry
/// exactly that. The `Pending` state distinguishes "not computed yet" from a
/// computed `NotCacheable`.
#[derive(Clone, Copy, Debug)]
enum NodeDigest {
    Pending,
    NotCacheable,
    Digest(Digest),
}

/// Identity of an external file an `FsPath` input points at, folded into the
/// digest so the same path holding different bytes invalidates the cache. The
/// default [`fs_file_id`] resolver uses `(len, mtime)` — cheap; a full content
/// hash is the opt-in for setups where mtime can lie.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FileId {
    pub(crate) len: u64,
    pub(crate) mtime_ns: u128,
}

/// `(len, mtime)` of `path`, or [`FileId::default`] when it can't be stat'd. A
/// missing file still differs from a present one, and the path string itself is
/// folded separately, so distinct missing paths stay distinct.
pub(crate) fn fs_file_id(path: &str) -> FileId {
    let Ok(meta) = std::fs::metadata(path) else {
        return FileId::default();
    };
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

/// Memoized digest computation, owning its per-node working columns so the
/// [`Cache`](crate::execution::cache::Cache) keeps one engine across updates and a
/// recompile reuses the buffers ([`reset`](Self::reset)) instead of reallocating two
/// graph-sized `Vec`s. The program is *not* held — it's passed to [`node_digest`](Self::node_digest)
/// per recompute, so the engine carries no lifetime. File identity is resolved with
/// [`fs_file_id`] directly.
#[derive(Debug, Default)]
pub(crate) struct DigestEngine {
    memo: Vec<NodeDigest>,
    /// Per-node in-progress marker — a node re-entered while still on the
    /// recursion stack is a cycle (the planner rejects these upstream; the digest
    /// pass treats it as non-reproducible rather than looping).
    visiting: Vec<bool>,
}

impl DigestEngine {
    /// Size the working columns to `program` and clear them — call once before the
    /// [`node_digest`](Self::node_digest) loop of a fresh recompute (the memo persists
    /// *within* that loop so a shared upstream node is hashed once).
    pub(crate) fn reset(&mut self, program: &ExecutionProgram) {
        let n = program.e_nodes.len();
        self.memo.clear();
        self.memo.resize(n, NodeDigest::Pending);
        self.visiting.clear();
        self.visiting.resize(n, false);
    }

    /// Content digest of node `idx`, or `None` when its output isn't reproducible
    /// — the node is `Impure`, or *any* upstream producer is non-reproducible (the
    /// taint flows up). `None` is the "always recompute, never cache" signal — for
    /// RAM and disk alike. Memoized.
    pub(crate) fn node_digest(
        &mut self,
        program: &ExecutionProgram,
        idx: NodeIdx,
    ) -> Option<Digest> {
        match self.memo[idx.idx()] {
            NodeDigest::Digest(d) => return Some(d),
            NodeDigest::NotCacheable => return None,
            NodeDigest::Pending => {}
        }
        // A file-cache node is keyed on its path input alone, *not* its input
        // cone — so input 0 can be impure/expensive and the node still presents a
        // digest (the reproducibility boundary). Resolve it before recursing.
        if matches!(
            program.e_nodes[idx].special,
            Some(SpecialNode::CachePassthrough { .. })
        ) {
            let e_node = &program.e_nodes[idx];
            let digest = file_cache_digest(program.node_inputs(e_node));
            self.memo[idx.idx()] = match digest {
                Some(d) => NodeDigest::Digest(d),
                None => NodeDigest::NotCacheable,
            };
            return digest;
        }

        if self.visiting[idx.idx()] {
            // Cycle (the planner rejects these) — treat as non-reproducible.
            return None;
        }
        self.visiting[idx.idx()] = true;

        let e_node = &program.e_nodes[idx];

        // Only a `Pure` node is content-cacheable; `Impure` varies per run.
        let digest = if e_node.behavior != FuncBehavior::Pure {
            None
        } else {
            let mut hasher = Hasher::new();
            hasher.update(DOMAIN);
            hasher.update(&e_node.func_id.as_u128().to_le_bytes());
            hasher.update(&e_node.func_version.to_le_bytes());

            // Output signature: arity + each resolved output type. A node that
            // produces a different shape (a flipped type, an added port) re-keys
            // here, so a stale blob can never be served under a key whose type no
            // longer matches — and the change propagates downstream through the
            // port digests below. A type change in a wildcard output already flows
            // in via its mirrored input; folding it again is harmless.
            let out_types = program.node_output_types(e_node);
            hasher.update(&(out_types.len() as u64).to_le_bytes());
            for ty in out_types {
                hash_data_type(&mut hasher, ty);
            }

            let mut tainted = false;
            for pool_idx in e_node.inputs.range() {
                match &program.inputs[pool_idx].binding {
                    // Unbound optional input — the runtime substitutes the func's
                    // `default_value`, which `func_version` stands in for, so a
                    // default change ships with a version bump.
                    ExecutionBinding::None => {
                        hasher.update(&[0u8]);
                    }
                    ExecutionBinding::Const(value) => {
                        hasher.update(&[1u8]);
                        Self::hash_static(&mut hasher, value);
                    }
                    ExecutionBinding::Bind(addr) => {
                        match self.port_digest(program, addr.target_idx, addr.port_idx) {
                            Some(upstream) => {
                                hasher.update(&[2u8]);
                                hasher.update(&upstream.0);
                            }
                            // A non-reproducible producer taints this node.
                            None => {
                                tainted = true;
                                break;
                            }
                        }
                    }
                }
            }
            (!tainted).then(|| Digest(hasher.finalize().into()))
        };

        self.visiting[idx.idx()] = false;
        self.memo[idx.idx()] = match digest {
            Some(d) => NodeDigest::Digest(d),
            None => NodeDigest::NotCacheable,
        };
        digest
    }

    /// Digest of one output *port* of node `idx`, or `None` if the node has no
    /// digest. Disambiguates ports of a multi-output node sharing one node digest.
    fn port_digest(
        &mut self,
        program: &ExecutionProgram,
        idx: NodeIdx,
        port_idx: usize,
    ) -> Option<Digest> {
        let node = self.node_digest(program, idx)?;
        let mut hasher = Hasher::new();
        hasher.update(&node.0);
        hasher.update(&(port_idx as u64).to_le_bytes());
        Some(Digest(hasher.finalize().into()))
    }

    /// Fold one constant into `hasher`: a discriminant tag plus length-prefixed
    /// payload (so `"ab"`+`"c"` can't collide with `"a"`+`"bc"`), and for an
    /// `FsPath` the resolved file identity on top of the path string.
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
                let id = fs_file_id(path);
                hasher.update(&id.len.to_le_bytes());
                hasher.update(&id.mtime_ns.to_le_bytes());
            }
            StaticValue::Enum(name) => {
                hasher.update(&[6u8]);
                hasher.update(&(name.len() as u64).to_le_bytes());
                hasher.update(name.as_bytes());
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::StaticValue;
    use crate::execution::program::{ExecutionInput, ExecutionNode, ExecutionPortAddress};
    use crate::function::FuncId;
    use crate::graph::NodeId;
    use common::Span;

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

    /// One node's digest with a one-shot engine (a fresh recompute), re-statting any
    /// `FsPath` const each call.
    fn digest_at(program: &ExecutionProgram, idx: NodeIdx) -> Option<Digest> {
        let mut engine = DigestEngine::default();
        engine.reset(program);
        engine.node_digest(program, idx)
    }

    fn digests(prog: &Prog) -> Vec<Option<Digest>> {
        let mut engine = DigestEngine::default();
        engine.reset(&prog.program);
        (0..prog.program.e_nodes.len())
            .map(|i| engine.node_digest(&prog.program, i.into()))
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

        let mut engine = DigestEngine::default();
        engine.reset(&p.program);
        assert_ne!(
            engine.port_digest(&p.program, NodeIdx(0), 0),
            engine.port_digest(&p.program, NodeIdx(0), 1),
            "ports of one node must hash apart"
        );
        let db = engine.node_digest(&p.program, NodeIdx(1));
        let dc = engine.node_digest(&p.program, NodeIdx(2));
        assert_ne!(db, dc, "consumers reading different ports must differ");
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
