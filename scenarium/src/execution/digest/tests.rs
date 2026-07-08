use super::*;
use crate::data::StaticValue;
use crate::execution::program::{ExecutionInput, ExecutionNode, ExecutionPortAddress};
use crate::graph::NodeId;
use crate::node::function::FuncId;
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
        let mut hasher = DigestHasher::new();
        hash_fs_path_identity(&mut hasher, p);
        hasher.finish()
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
        let outputs_start = self.program.output_types.len() as u32;
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
fn digested_cache(program: &ExecutionProgram, through: NodeIdx) -> RuntimeCache {
    let mut cache = RuntimeCache::default();
    cache.reconcile(&program.e_nodes);
    for idx in program.node_indices().take(through.idx() + 1) {
        let d = node_digest(program, idx, &cache);
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

    // write_digest folds the nested digest's raw 32 bytes — same as write_bytes(&inner.0).
    let inner = {
        let mut h = DigestHasher::new();
        h.write_bytes(b"inner");
        h.finish()
    };
    assert_eq!(
        hash_with(&|h| {
            h.write_digest(&inner);
        }),
        hash_with(&|h| {
            h.write_bytes(&inner.0);
        }),
        "write_digest folds the digest's raw bytes"
    );
}
