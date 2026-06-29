//! Content digests for node outputs — the cache key for the per-slot RAM cache
//! (and the disk cache, when that's wired up).
//!
//! A node's output is a pure function of its function (identity + version), its
//! resolved input values, the outputs of its upstream producers, and the content
//! of any external files it reads. [`DigestEngine`] folds exactly that into a
//! 256-bit BLAKE3 digest over the flattened [`ExecutionProgram`], memoized per
//! node. Equal digests ⇒ identical computation, so the digest is at once the
//! cache key *and* the invalidation signal: change anything upstream and every
//! downstream digest changes — on this machine or any other. See
//! `README.md` Part B.

use blake3::Hasher;

use crate::data::{DataType, StaticValue};
use crate::elements::cache_passthrough::file_cache_digest;
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::function::FuncBehavior;
use crate::special::SpecialNode;

/// Domain separator mixed into every node digest. Bump the suffix to invalidate
/// every cached digest when the hashing scheme itself changes.
const DOMAIN: &[u8] = b"scenarium-cache-v1";

/// 256-bit content digest. Cross-machine stable for a given binary: equal
/// digests mean the same func+version, params, upstream outputs, and file inputs.
pub(crate) type Digest = [u8; 32];

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

/// Memoized digest computation over one [`ExecutionProgram`]. Reuse a single
/// engine for the whole program so a shared upstream node is hashed once.
pub(crate) struct DigestEngine<'a, F> {
    program: &'a ExecutionProgram,
    /// Each node's resolved output types (one list per node, indexed by node), folded
    /// into its digest so redefining a func's outputs (`Int → Float`, an added port)
    /// re-keys the cache without relying on a `func_version` bump. Resolved once at
    /// `update` ([`Cache::recompute_digests`](crate::execution::cache::Cache::recompute_digests)).
    output_types: &'a [Vec<DataType>],
    file_id: F,
    memo: Vec<NodeDigest>,
    /// Per-node in-progress marker — a node re-entered while still on the
    /// recursion stack is a cycle (the planner rejects these upstream; the digest
    /// pass treats it as non-reproducible rather than looping).
    visiting: Vec<bool>,
}

// Manual `Debug` (the rule is "always derive Debug"): the `file_id` resolver is
// an `F: Fn` closure that isn't `Debug`, so the derive can't apply.
impl<F> std::fmt::Debug for DigestEngine<'_, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DigestEngine")
            .field("memo", &self.memo)
            .field("visiting", &self.visiting)
            .finish_non_exhaustive()
    }
}

impl<'a> DigestEngine<'a, fn(&str) -> FileId> {
    /// Engine using the filesystem [`fs_file_id`] resolver — the production path.
    pub(crate) fn with_fs(
        program: &'a ExecutionProgram,
        output_types: &'a [Vec<DataType>],
    ) -> Self {
        DigestEngine::new(program, output_types, fs_file_id)
    }
}

impl<'a, F: Fn(&str) -> FileId> DigestEngine<'a, F> {
    pub(crate) fn new(
        program: &'a ExecutionProgram,
        output_types: &'a [Vec<DataType>],
        file_id: F,
    ) -> Self {
        let n = program.e_nodes.len();
        Self {
            program,
            output_types,
            file_id,
            memo: vec![NodeDigest::Pending; n],
            visiting: vec![false; n],
        }
    }

    /// Content digest of node `idx`, or `None` when its output isn't reproducible
    /// — the node is `Impure`, or *any* upstream producer is non-reproducible (the
    /// taint flows up). `None` is the "always recompute, never cache" signal — for
    /// RAM and disk alike. Memoized.
    pub(crate) fn node_digest(&mut self, idx: usize) -> Option<Digest> {
        match self.memo[idx] {
            NodeDigest::Digest(d) => return Some(d),
            NodeDigest::NotCacheable => return None,
            NodeDigest::Pending => {}
        }
        // A file-cache node is keyed on its path input alone, *not* its input
        // cone — so input 0 can be impure/expensive and the node still presents a
        // digest (the reproducibility boundary). Resolve it before recursing.
        if matches!(
            self.program.e_nodes[idx].special,
            Some(SpecialNode::CachePassthrough { .. })
        ) {
            let e_node = &self.program.e_nodes[idx];
            let digest = file_cache_digest(self.program.node_inputs(e_node));
            self.memo[idx] = match digest {
                Some(d) => NodeDigest::Digest(d),
                None => NodeDigest::NotCacheable,
            };
            return digest;
        }

        if self.visiting[idx] {
            // Cycle (the planner rejects these) — treat as non-reproducible.
            return None;
        }
        self.visiting[idx] = true;

        // Copy the shared references out so the input loop borrows them (lifetime
        // 'a), leaving `self` free for the recursive call.
        let program = self.program;
        let output_types = self.output_types;
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
            let out_types = &output_types[idx];
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
                        Self::hash_static(&mut hasher, value, &self.file_id);
                    }
                    ExecutionBinding::Bind(addr) => {
                        match self.port_digest(addr.target_idx, addr.port_idx) {
                            Some(upstream) => {
                                hasher.update(&[2u8]);
                                hasher.update(&upstream);
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
            (!tainted).then(|| hasher.finalize().into())
        };

        self.visiting[idx] = false;
        self.memo[idx] = match digest {
            Some(d) => NodeDigest::Digest(d),
            None => NodeDigest::NotCacheable,
        };
        digest
    }

    /// Digest of one output *port* of node `idx`, or `None` if the node has no
    /// digest. Disambiguates ports of a multi-output node sharing one node digest.
    pub(crate) fn port_digest(&mut self, idx: usize, port_idx: usize) -> Option<Digest> {
        let node = self.node_digest(idx)?;
        let mut hasher = Hasher::new();
        hasher.update(&node);
        hasher.update(&(port_idx as u64).to_le_bytes());
        Some(hasher.finalize().into())
    }

    /// Fold one constant into `hasher`: a discriminant tag plus length-prefixed
    /// payload (so `"ab"`+`"c"` can't collide with `"a"`+`"bc"`), and for an
    /// `FsPath` the resolved file identity on top of the path string.
    fn hash_static(hasher: &mut Hasher, value: &StaticValue, file_id: &F) {
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
                let id = file_id(path);
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
    /// `from_u128(idx + 1)`; `bind`'s target id must match that scheme. `output_types`
    /// parallels the nodes — each output defaults to `Int`, overridable via
    /// [`Prog::add_typed`] to exercise the output-signature folding.
    #[derive(Debug, Default)]
    struct Prog {
        program: ExecutionProgram,
        output_types: Vec<Vec<DataType>>,
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
            self.add_with(FuncBehavior::Pure, func, version, outputs, bindings)
        }

        /// Add a `Pure` node with explicit output types (the digest folds them).
        fn add_typed(
            &mut self,
            func: u128,
            types: &[DataType],
            bindings: &[ExecutionBinding],
        ) -> usize {
            let idx = self.add_with(FuncBehavior::Pure, func, 0, types.len() as u32, bindings);
            self.output_types[idx] = types.to_vec();
            idx
        }

        /// Add an `Impure` node — its `node_digest` is always `None`.
        fn add_impure(&mut self, func: u128, outputs: u32, bindings: &[ExecutionBinding]) -> usize {
            self.add_with(FuncBehavior::Impure, func, 0, outputs, bindings)
        }

        fn add_with(
            &mut self,
            behavior: FuncBehavior,
            func: u128,
            version: u64,
            outputs: u32,
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
            self.program.n_outputs += outputs as usize;
            self.program.e_nodes.add(ExecutionNode {
                id: NodeId::from_u128(idx as u128 + 1),
                inited: true,
                behavior,
                func_id: FuncId::from_u128(func),
                func_version: version,
                inputs: Span::new(inputs_start, bindings.len() as u32),
                outputs: Span::new(outputs_start, outputs),
                ..Default::default()
            });
            self.output_types
                .push(vec![DataType::Int; outputs as usize]);
            idx
        }
    }

    fn bind(idx: usize, port: usize) -> ExecutionBinding {
        ExecutionBinding::Bind(ExecutionPortAddress {
            target_idx: idx,
            port_idx: port,
        })
    }

    fn konst(value: StaticValue) -> ExecutionBinding {
        ExecutionBinding::Const(value)
    }

    fn no_files(_: &str) -> FileId {
        FileId::default()
    }

    fn digests(prog: &Prog) -> Vec<Option<Digest>> {
        let mut engine = DigestEngine::new(&prog.program, &prog.output_types, no_files);
        (0..prog.program.e_nodes.len())
            .map(|i| engine.node_digest(i))
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
        // Same FsPath const, different file identity (e.g. mtime) ⇒ different
        // digest: this is what stops machine B serving A's result for B's files.
        let mut p = Prog::default();
        p.add(10, 0, 1, &[konst(StaticValue::FsPath("frames".into()))]);
        let t = &p.output_types;

        let d_small = DigestEngine::new(&p.program, t, |_: &str| FileId {
            len: 1,
            mtime_ns: 1,
        })
        .node_digest(0);
        let d_large = DigestEngine::new(&p.program, t, |_: &str| FileId {
            len: 2,
            mtime_ns: 1,
        })
        .node_digest(0);
        let d_mtime = DigestEngine::new(&p.program, t, |_: &str| FileId {
            len: 1,
            mtime_ns: 9,
        })
        .node_digest(0);
        assert_ne!(d_small, d_large, "file length must matter");
        assert_ne!(d_small, d_mtime, "file mtime must matter");

        let same = DigestEngine::new(&p.program, t, |_: &str| FileId {
            len: 1,
            mtime_ns: 1,
        })
        .node_digest(0);
        assert_eq!(d_small, same, "identical file identity ⇒ identical digest");

        // The path string itself is folded too, independent of file identity.
        let mut q = Prog::default();
        q.add(10, 0, 1, &[konst(StaticValue::FsPath("other".into()))]);
        let d_other = DigestEngine::new(&q.program, &q.output_types, |_: &str| FileId {
            len: 1,
            mtime_ns: 1,
        })
        .node_digest(0);
        assert_ne!(d_small, d_other, "different path ⇒ different digest");
    }

    #[test]
    fn output_ports_are_disambiguated() {
        // One producer with two outputs; consumers on different ports differ.
        let mut p = Prog::default();
        p.add(10, 0, 2, &[]); // A, two outputs
        p.add(20, 0, 1, &[bind(0, 0)]); // B binds A.0
        p.add(20, 0, 1, &[bind(0, 1)]); // C binds A.1 (same func as B)

        let mut engine = DigestEngine::new(&p.program, &p.output_types, no_files);
        assert_ne!(
            engine.port_digest(0, 0),
            engine.port_digest(0, 1),
            "ports of one node must hash apart"
        );
        let db = engine.node_digest(1);
        let dc = engine.node_digest(2);
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
        assert_eq!(
            DigestEngine::new(&p.program, &p.output_types, no_files).node_digest(0),
            None
        );
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
