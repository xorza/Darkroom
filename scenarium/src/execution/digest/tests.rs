use super::*;
use crate::StaticValue;
use crate::execution::cache::test_support::hydrate;
use crate::execution::cache::{CachedOutputCoverage, OutputSnapshot};
use crate::execution::program::{ExecutionInput, ExecutionNode, ExecutionPortAddress};
use crate::graph::NodeId;
use crate::node::definition::FuncId;
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

    // An empty directory and an unreadable one must key apart — otherwise a
    // value cached against "empty" keeps being served after the directory
    // turns unreadable (the sentinel-count fix in `hash_dir_entries`).
    #[cfg(unix)]
    {
        use std::fs::Permissions;
        use std::os::unix::fs::PermissionsExt;
        let empty = fingerprint(&path);
        let perms = |mode: u32| Permissions::from_mode(mode);
        std::fs::set_permissions(&dir, perms(0o000)).unwrap();
        let unreadable = fingerprint(&path);
        std::fs::set_permissions(&dir, perms(0o755)).unwrap();
        assert_ne!(
            unreadable, empty,
            "an unreadable directory keys apart from an empty one"
        );
        assert_eq!(fingerprint(&path), empty, "readable again folds as before");
    }

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

    /// Mark input `input_idx` of node `idx` resource-typed with `stamper` (inputs default
    /// to none) — gates the Bind-side referent-identity fold.
    fn stamp_input(&mut self, idx: usize, input_idx: usize, stamper: InputStamper) {
        let pool = self.program.e_nodes[&node_id(idx)].inputs.start as usize + input_idx;
        self.program.inputs[pool].stamper = Some(stamper);
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
                stamper: None,
                binding: binding.clone(),
            });
        }
        let idx = self.program.e_nodes.len();
        let outputs_start = self.program.output_types.len() as u32;
        self.program.output_types.extend_from_slice(types);
        let node_id = node_id(idx);
        self.program.node_order.push(node_id);
        self.program.e_nodes.insert(
            node_id,
            ExecutionNode {
                id: node_id,
                behavior,
                func_id: FuncId::from_u128(func),
                func_version: version,
                inputs: Span::new(inputs_start, bindings.len() as u32),
                outputs: Span::new(outputs_start, types.len() as u32),
                ..Default::default()
            },
        );
        idx
    }
}

fn bind(idx: usize, port: usize) -> ExecutionBinding {
    ExecutionBinding::Bind(ExecutionPortAddress {
        target: node_id(idx),
        port_idx: port,
    })
}

fn node_id(idx: usize) -> NodeId {
    NodeId::from_u128(idx as u128 + 1)
}

fn konst(value: StaticValue) -> ExecutionBinding {
    ExecutionBinding::Const(value)
}

/// Fold node digests into a fresh cache the way the executor does — producer-first
/// in `e_node` order (the test `Prog`s are built that way), each node reading its
/// producers' just-stamped `current_digest` — stopping after `through`. Re-stats any
/// `FsPath` const each call. Returns the cache, holding every computed digest.
fn digested_cache(program: &ExecutionProgram, through: usize) -> RuntimeCache {
    let mut cache = RuntimeCache::default();
    cache.reconcile(program);
    for node_id in program.node_ids().take(through + 1) {
        let digest = node_digest(program, node_id, &cache);
        cache.slots.get_mut(&node_id).unwrap().current_digest = digest;
    }
    cache
}

/// One node's content digest, computing only the producer-first prefix it needs.
fn digest_at(program: &ExecutionProgram, idx: usize) -> Option<Digest> {
    digested_cache(program, idx).slots[&node_id(idx)].current_digest
}

/// Every node's content digest, indexed by position.
fn digests(prog: &Prog) -> Vec<Option<Digest>> {
    let last = prog.program.e_nodes.len().saturating_sub(1);
    let cache = digested_cache(&prog.program, last);
    prog.program
        .node_ids()
        .map(|node_id| cache.slots[&node_id].current_digest)
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
    let d_len1 = digest_at(&p.program, 0);
    std::fs::write(&file, b"xyz").unwrap(); // len 3 — file identity changed
    let d_len3 = digest_at(&p.program, 0);
    assert_ne!(
        d_len1, d_len3,
        "a file content change must re-key the digest"
    );

    // A present file and a missing one are distinct identities.
    std::fs::remove_file(&file).unwrap();
    let d_missing = digest_at(&p.program, 0);
    assert_ne!(d_len3, d_missing, "file presence must matter");

    // The path string itself is folded, independent of file identity (both missing).
    let d_other = digest_at(&prog_for("definitely-missing-elsewhere").program, 0);
    assert_ne!(d_missing, d_other, "different path ⇒ different digest");
}

/// A **Bind-delivered** path re-keys its consumer like a const one: the fold reads the
/// producer's delivered value and stats the pointed-at file live — but only through an
/// `FsPath`-declared input, and only once the value is readable (unreadable ⇒ `None`,
/// the taint the run loop's reach-time re-stamp resolves).
#[test]
fn bound_fs_path_folds_delivered_file_identity() {
    use crate::DynamicValue;

    let file = std::env::temp_dir().join(format!(
        "scenarium-digest-bound-fs-{}.bin",
        std::process::id()
    ));
    let path = file.to_string_lossy().into_owned();

    // producer (0) → consumer (1) with its input declared `FsPath`; a control consumer (2)
    // reads the same port through an undeclared input — no fold.
    let mut p = Prog::default();
    p.add(10, 0, 1, &[]);
    p.add(20, 0, 1, &[bind(0, 0)]);
    p.add(20, 0, 1, &[bind(0, 0)]);
    p.stamp_input(1, 0, InputStamper::FsPath);

    // Stamp the producer and install `value` as its delivered output (`None` leaves the
    // slot empty — an unreadable value), then fold both consumers.
    let digests_with = |value: Option<DynamicValue>| {
        let mut cache = RuntimeCache::default();
        cache.reconcile(&p.program);
        let producer = node_digest(&p.program, node_id(0), &cache).unwrap();
        cache.slots.get_mut(&node_id(0)).unwrap().current_digest = Some(producer);
        if let Some(value) = value {
            hydrate(
                &mut cache,
                node_id(0),
                OutputSnapshot::new(vec![value], CachedOutputCoverage { ports: vec![true] }),
                producer,
            );
        }
        (
            node_digest(&p.program, node_id(1), &cache),
            node_digest(&p.program, node_id(2), &cache),
        )
    };
    let fs_path = || Some(DynamicValue::Static(StaticValue::FsPath(path.clone())));

    std::fs::write(&file, b"x").unwrap(); // len 1
    let (typed_len1, plain_len1) = digests_with(fs_path());
    assert!(typed_len1.is_some() && plain_len1.is_some());
    assert_eq!(
        digests_with(fs_path()).0,
        typed_len1,
        "an unchanged file folds identically"
    );

    std::fs::write(&file, b"xyz").unwrap(); // len 3 — the file identity changed
    let (typed_len3, plain_len3) = digests_with(fs_path());
    assert_ne!(
        typed_len1, typed_len3,
        "a wired path's file change re-keys the FsPath-declared consumer"
    );
    assert_eq!(
        plain_len1, plain_len3,
        "an undeclared input folds no file identity — structural digest only"
    );

    std::fs::remove_file(&file).unwrap();
    let (typed_missing, _) = digests_with(fs_path());
    assert_ne!(typed_len3, typed_missing, "file presence must matter");

    // A delivered non-path value folds a distinct marker — still cacheable.
    let (typed_int, _) = digests_with(Some(DynamicValue::Static(StaticValue::Int(7))));
    assert!(typed_int.is_some(), "a mis-typed wire stays cacheable");
    assert_ne!(
        typed_int, typed_missing,
        "a non-path delivered value hashes apart from a missing path"
    );

    // An unreadable delivered value (producer not resident) taints only the declared consumer.
    let (typed_unread, plain_unread) = digests_with(None);
    assert_eq!(
        typed_unread, None,
        "unreadable reference value ⇒ None digest"
    );
    assert!(
        plain_unread.is_some(),
        "the undeclared consumer never reads the value, so it still folds"
    );
}

/// A registered [`ResourceStamper`] keys a consumer on the *referent's* state, not the
/// reference value: bumping the external version re-keys the stamped consumer while the
/// producer and an unstamped sibling stay put — the stamp is read live at every fold.
#[test]
fn custom_stamper_folds_referent_version() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::{DynamicValue, ResourceStamp, ResourceStamper};

    #[derive(Debug)]
    struct VersionStamper(Arc<AtomicU64>);
    impl ResourceStamper for VersionStamper {
        fn stamp(&self, _value: &DynamicValue) -> ResourceStamp {
            ResourceStamp::from_bytes(&self.0.load(Ordering::SeqCst).to_le_bytes())
        }
    }

    let version = Arc::new(AtomicU64::new(1));

    // producer (0) → stamped consumer (1); control consumer (2) reads the same port
    // unstamped. The delivered value is an ordinary resident value — what identity it
    // yields is the stamper's business, not the framework's.
    let mut p = Prog::default();
    p.add(10, 0, 1, &[]);
    p.add(20, 0, 1, &[bind(0, 0)]);
    p.add(20, 0, 1, &[bind(0, 0)]);
    p.stamp_input(
        1,
        0,
        InputStamper::Custom(Arc::new(VersionStamper(version.clone()))),
    );

    let digests = || {
        let mut cache = RuntimeCache::default();
        cache.reconcile(&p.program);
        let producer = node_digest(&p.program, node_id(0), &cache).unwrap();
        cache.slots.get_mut(&node_id(0)).unwrap().current_digest = Some(producer);
        hydrate(
            &mut cache,
            node_id(0),
            OutputSnapshot::new(
                vec![StaticValue::Int(42).into()],
                CachedOutputCoverage { ports: vec![true] },
            ),
            producer,
        );
        (
            node_digest(&p.program, node_id(1), &cache),
            node_digest(&p.program, node_id(2), &cache),
        )
    };

    let (stamped_v1, plain_v1) = digests();
    assert!(stamped_v1.is_some());
    assert_eq!(
        digests().0,
        stamped_v1,
        "an unchanged referent folds identically"
    );

    version.store(2, Ordering::SeqCst);
    let (stamped_v2, plain_v2) = digests();
    assert_ne!(
        stamped_v1, stamped_v2,
        "a referent version bump re-keys the stamped consumer"
    );
    assert_eq!(plain_v1, plain_v2, "the unstamped sibling is untouched");
}

#[test]
fn output_ports_are_disambiguated() {
    // One producer with two outputs; consumers on different ports differ.
    let mut p = Prog::default();
    p.add(10, 0, 2, &[]); // A, two outputs
    p.add(20, 0, 1, &[bind(0, 0)]); // B binds A.0
    p.add(20, 0, 1, &[bind(0, 1)]); // C binds A.1 (same func as B)

    let a = digest_at(&p.program, 0).unwrap();
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
    use crate::TypeId;

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
    assert_eq!(digest_at(&p.program, 0), None);
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
