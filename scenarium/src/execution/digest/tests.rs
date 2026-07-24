use super::*;
use crate::StaticValue;
use crate::execution::cache::runtime::test_support::hydrate;
use crate::execution::cache::slot::OutputSnapshot;
use crate::execution::identity::{ExecutionNodeId, ExecutionOutputPort};
use crate::execution::program::{ExecutionInput, ExecutionNode, ExecutionOutput};
use crate::execution::resource::RunResourceStamps;
use crate::execution::resource::test_support::prepare_node;
use crate::node::definition::FuncId;

/// Minimal hand-built `ExecutionProgram` for digest tests. Node ids are
/// `from_u128(idx + 1)`; `bind`'s target id must match that scheme. Output types
/// go straight into the packed output metadata — each output defaults to `Int`,
/// overridable via [`Prog::add_typed`] to exercise the output-signature folding.
#[derive(Debug, Default)]
struct Prog {
    program: ExecutionProgram,
}

impl Prog {
    /// Add a `Pure` (content-cacheable) node; outputs default to `Int`.
    fn add(&mut self, func: u128, outputs: u32, bindings: &[ExecutionBinding]) -> usize {
        self.add_with(
            FuncBehavior::Pure,
            func,
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
        self.add_with(FuncBehavior::Pure, func, types, bindings)
    }

    /// Add an `Impure` node — its `node_digest` is always `None`.
    fn add_impure(&mut self, func: u128, outputs: u32, bindings: &[ExecutionBinding]) -> usize {
        self.add_with(
            FuncBehavior::Impure,
            func,
            &vec![DataType::Int; outputs as usize],
            bindings,
        )
    }

    /// Mark input `input_idx` of node `idx` as a declared filesystem-path input.
    fn stamp_fs_path_input(&mut self, idx: usize, input_idx: usize) {
        let pool = self.program.e_nodes[&e_node_id(idx)].inputs.start as usize + input_idx;
        self.program.inputs[pool].stamps_fs_path = true;
    }

    fn add_with(
        &mut self,
        behavior: FuncBehavior,
        func: u128,
        types: &[DataType],
        bindings: &[ExecutionBinding],
    ) -> usize {
        let inputs = self
            .program
            .inputs
            .append(bindings.iter().map(|binding| ExecutionInput {
                required: false,
                stamps_fs_path: false,
                binding: binding.clone(),
            }));
        let idx = self.program.e_nodes.len();
        let outputs = self
            .program
            .outputs
            .append(types.iter().cloned().map(|data_type| ExecutionOutput {
                data_type,
                pinned: false,
            }));
        let e_node_id = e_node_id(idx);
        self.program.e_nodes.insert(
            e_node_id,
            ExecutionNode {
                behavior,
                func_id: FuncId::from_u128(func),
                inputs,
                outputs,
                ..Default::default()
            },
        );
        idx
    }
}

fn bind(idx: usize, port: usize) -> ExecutionBinding {
    ExecutionBinding::Bind(ExecutionOutputPort {
        e_node_id: e_node_id(idx),
        port_idx: port,
    })
}

fn e_node_id(idx: usize) -> ExecutionNodeId {
    ExecutionNodeId::from_u128(idx as u128 + 1)
}

fn konst(value: StaticValue) -> ExecutionBinding {
    ExecutionBinding::Const(value)
}

#[derive(Debug)]
struct DigestPair {
    typed: Option<Digest>,
    plain: Option<Digest>,
}

/// Fold node digests into a fresh cache the way the executor does — producer-first
/// in fixture index order, each node reading its
/// producers' just-stamped `current_digest` — stopping after `through`. Prepares a fresh
/// resource stamps each call. Returns the cache, holding every computed digest.
fn digested_cache(program: &ExecutionProgram, through: usize) -> RuntimeCache {
    let mut cache = RuntimeCache::default();
    cache.reconcile(program);
    let mut resource_stamps = RunResourceStamps::default();
    for idx in 0..=through {
        let e_node_id = e_node_id(idx);
        prepare_node(&mut resource_stamps, program, &cache, e_node_id);
        let digest = node_digest(program, e_node_id, &cache, &resource_stamps);
        cache.slots.get_mut(&e_node_id).unwrap().current_digest = digest;
    }
    cache
}

/// One node's content digest, computing only the producer-first prefix it needs.
fn digest_at(program: &ExecutionProgram, idx: usize) -> Option<Digest> {
    digested_cache(program, idx).slots[&e_node_id(idx)].current_digest
}

/// Every node's content digest, indexed by position.
fn digests(prog: &Prog) -> Vec<Option<Digest>> {
    let last = prog.program.e_nodes.len().saturating_sub(1);
    let cache = digested_cache(&prog.program, last);
    (0..prog.program.e_nodes.len())
        .map(|idx| cache.slots[&e_node_id(idx)].current_digest)
        .collect()
}

#[test]
fn deterministic_and_per_function_distinct() {
    // A → B (B binds A.0), plus an independent C with a const input.
    let mut p = Prog::default();
    p.add(10, 1, &[]); // A
    p.add(20, 1, &[bind(0, 0)]); // B
    p.add(30, 1, &[konst(StaticValue::Int(5))]); // C

    let first = digests(&p);
    let second = digests(&p); // same engine inputs → identical digests
    assert_eq!(first, second, "digest must be deterministic");

    // Distinct functions ⇒ distinct digests (no accidental collisions).
    assert_ne!(first[0], first[1]);
    assert_ne!(first[1], first[2]);
    assert_ne!(first[0], first[2]);

    p.program.e_nodes.get_mut(&e_node_id(0)).unwrap().version = 1;
    let versioned = digests(&p);
    assert_ne!(
        first[0], versioned[0],
        "a function version re-keys its node"
    );
    assert_ne!(
        first[1], versioned[1],
        "a function version propagates downstream"
    );
    assert_eq!(
        first[2], versioned[2],
        "an independent node ignores another function's version"
    );
}

#[test]
fn const_change_propagates_downstream_only() {
    let build = |a_const: i64| {
        let mut p = Prog::default();
        p.add(10, 1, &[konst(StaticValue::Int(a_const))]); // A
        p.add(20, 1, &[bind(0, 0)]); // B binds A
        p.add(30, 1, &[konst(StaticValue::Int(9))]); // C, independent
        p
    };
    let base = digests(&build(1));
    let changed = digests(&build(2));

    assert_ne!(base[0], changed[0], "A's own digest tracks its const");
    assert_ne!(base[1], changed[1], "B downstream of A must change too");
    assert_eq!(base[2], changed[2], "independent C is unaffected");
}

#[test]
fn structurally_identical_nodes_share_digest() {
    // Two nodes, same func, same (input-identical) bindings ⇒ equal
    // node digests — the property that lets the store dedup repeated work.
    let mut p = Prog::default();
    p.add(10, 1, &[konst(StaticValue::Int(7))]);
    p.add(10, 1, &[konst(StaticValue::Int(7))]);
    let d = digests(&p);
    assert_eq!(d[0], d[1]);

    // Differ in func or const ⇒ digests diverge.
    let mut q = Prog::default();
    q.add(10, 1, &[konst(StaticValue::Int(7))]);
    q.add(11, 1, &[konst(StaticValue::Int(7))]); // different func
    q.add(10, 1, &[konst(StaticValue::Int(8))]); // different const
    let dq = digests(&q);
    assert_ne!(dq[0], dq[1]);
    assert_ne!(dq[0], dq[2]);
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
        p.add(10, 1, &[konst(StaticValue::FsPath(path.into()))]);
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

    let unselected = file.with_file_name("scenarium_digest_unselected.bin");
    std::fs::write(&unselected, b"not selected").unwrap();
    assert_eq!(
        digest_at(&p.program, 0),
        d_len3,
        "an unselected sibling file must not affect a single-path digest"
    );

    let second = file.with_file_name("scenarium_digest_selected_second.bin");
    std::fs::write(&second, b"second").unwrap();
    let mut selected = Prog::default();
    selected.add(
        10,
        1,
        &[konst(StaticValue::FsPaths(vec![
            path.clone(),
            second.to_string_lossy().into_owned(),
        ]))],
    );
    let two_files = digest_at(&selected.program, 0);
    std::fs::write(&second, b"second changed").unwrap();
    let second_edited = digest_at(&selected.program, 0);
    assert_ne!(
        two_files, second_edited,
        "editing any selected file must re-key the list"
    );

    let mut reversed = Prog::default();
    reversed.add(
        10,
        1,
        &[konst(StaticValue::FsPaths(vec![
            second.to_string_lossy().into_owned(),
            path.clone(),
        ]))],
    );
    assert_ne!(
        digest_at(&reversed.program, 0),
        second_edited,
        "path-list order is part of the authored input"
    );

    // A present file and a missing one are distinct identities.
    std::fs::remove_file(&file).unwrap();
    let d_missing = digest_at(&p.program, 0);
    assert_ne!(d_len3, d_missing, "file presence must matter");

    // The path string itself is folded, independent of file identity (both missing).
    let d_other = digest_at(&prog_for("definitely-missing-elsewhere").program, 0);
    assert_eq!(
        d_other,
        Some(Digest([
            176, 232, 89, 255, 138, 89, 119, 40, 183, 103, 129, 186, 168, 197, 234, 18, 53, 132,
            24, 131, 229, 252, 228, 149, 13, 238, 235, 109, 166, 65, 110, 61,
        ])),
        "the single-path digest encoding must remain stable"
    );
    let mut singleton_list = Prog::default();
    singleton_list.add(
        10,
        1,
        &[konst(StaticValue::FsPaths(vec![
            "definitely-missing-elsewhere".into(),
        ]))],
    );
    assert_ne!(
        digest_at(&singleton_list.program, 0),
        d_other,
        "single-path and path-list variants must hash apart"
    );
    assert_ne!(d_missing, d_other, "different path ⇒ different digest");
    std::fs::remove_file(unselected).unwrap();
    std::fs::remove_file(second).unwrap();
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
    p.add(10, 1, &[]);
    p.add(20, 1, &[bind(0, 0)]);
    p.add(20, 1, &[bind(0, 0)]);
    p.stamp_fs_path_input(1, 0);

    // Stamp the producer and install `value` as its delivered output (`None` leaves the
    // slot empty — an unreadable value), then fold both consumers.
    let digests_with = |value: Option<DynamicValue>| {
        let mut cache = RuntimeCache::default();
        cache.reconcile(&p.program);
        let mut resource_stamps = RunResourceStamps::default();
        let producer = node_digest(&p.program, e_node_id(0), &cache, &resource_stamps).unwrap();
        cache.slots.get_mut(&e_node_id(0)).unwrap().current_digest = Some(producer);
        if let Some(value) = value {
            hydrate(
                &mut cache,
                e_node_id(0),
                OutputSnapshot::new(vec![value]),
                producer,
            );
        }
        prepare_node(&mut resource_stamps, &p.program, &cache, e_node_id(1));
        prepare_node(&mut resource_stamps, &p.program, &cache, e_node_id(2));
        DigestPair {
            typed: node_digest(&p.program, e_node_id(1), &cache, &resource_stamps),
            plain: node_digest(&p.program, e_node_id(2), &cache, &resource_stamps),
        }
    };
    let fs_path = || Some(DynamicValue::Static(StaticValue::FsPath(path.clone())));

    std::fs::write(&file, b"x").unwrap(); // len 1
    let DigestPair {
        typed: typed_len1,
        plain: plain_len1,
    } = digests_with(fs_path());
    assert!(typed_len1.is_some() && plain_len1.is_some());
    assert_eq!(
        digests_with(fs_path()).typed,
        typed_len1,
        "an unchanged file folds identically"
    );

    std::fs::write(&file, b"xyz").unwrap(); // len 3 — the file identity changed
    let DigestPair {
        typed: typed_len3,
        plain: plain_len3,
    } = digests_with(fs_path());
    assert_ne!(
        typed_len1, typed_len3,
        "a wired path's file change re-keys the FsPath-declared consumer"
    );
    assert_eq!(
        plain_len1, plain_len3,
        "an undeclared input folds no file identity — structural digest only"
    );

    let second = file.with_file_name(format!(
        "scenarium-digest-bound-fs-second-{}.bin",
        std::process::id()
    ));
    std::fs::write(&second, b"second").unwrap();
    let fs_paths = || {
        Some(DynamicValue::Static(StaticValue::FsPaths(vec![
            path.clone(),
            second.to_string_lossy().into_owned(),
        ])))
    };
    let typed_list = digests_with(fs_paths()).typed;
    std::fs::write(&second, b"second changed").unwrap();
    assert_ne!(
        digests_with(fs_paths()).typed,
        typed_list,
        "a wired path list re-keys when any selected file changes"
    );
    std::fs::remove_file(second).unwrap();

    std::fs::remove_file(&file).unwrap();
    let typed_missing = digests_with(fs_path()).typed;
    assert_ne!(typed_len3, typed_missing, "file presence must matter");

    // A delivered non-path value folds a distinct marker — still cacheable.
    let typed_int = digests_with(Some(DynamicValue::Static(StaticValue::Int(7)))).typed;
    assert!(typed_int.is_some(), "a mis-typed wire stays cacheable");
    assert_ne!(
        typed_int, typed_missing,
        "a non-path delivered value hashes apart from a missing path"
    );

    // An unreadable delivered value (producer not resident) taints only the declared consumer.
    let DigestPair {
        typed: typed_unread,
        plain: plain_unread,
    } = digests_with(None);
    assert_eq!(
        typed_unread, None,
        "unreadable reference value ⇒ None digest"
    );
    assert!(
        plain_unread.is_some(),
        "the undeclared consumer never reads the value, so it still folds"
    );

    let mut cache = RuntimeCache::default();
    cache.reconcile(&p.program);
    let mut resource_stamps = RunResourceStamps::default();
    let producer = node_digest(&p.program, e_node_id(0), &cache, &resource_stamps).unwrap();
    cache.slots.get_mut(&e_node_id(0)).unwrap().current_digest = Some(producer);
    hydrate(
        &mut cache,
        e_node_id(0),
        OutputSnapshot::new(vec![DynamicValue::Static(StaticValue::FsPath(path))]),
        producer,
    );
    cache.slots.get_mut(&e_node_id(0)).unwrap().current_digest = Some(Digest([9; 32]));
    prepare_node(&mut resource_stamps, &p.program, &cache, e_node_id(1));
    assert_eq!(
        node_digest(&p.program, e_node_id(1), &cache, &resource_stamps),
        None,
        "a path value produced under an old producer digest is unreadable"
    );
}

#[test]
fn output_ports_are_disambiguated() {
    // One producer with two outputs; consumers on different ports differ.
    let mut p = Prog::default();
    p.add(10, 2, &[]); // A, two outputs
    p.add(20, 1, &[bind(0, 0)]); // B binds A.0
    p.add(20, 1, &[bind(0, 1)]); // C binds A.1 (same func as B)

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
/// type.
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
    p.add(10, 1, &[bind(1, 0)]); // A binds B (idx 1)
    p.add(20, 1, &[bind(0, 0)]); // B binds A (idx 0)
    assert_eq!(digest_at(&p.program, 0), None);
}

#[test]
fn impure_node_and_its_dependents_are_none() {
    // src (impure) → mid (pure) → sink (pure). The impure source taints the
    // whole downstream chain; an independent pure node stays `Some`.
    let mut p = Prog::default();
    p.add_impure(10, 1, &[]); // 0: impure source
    p.add(20, 1, &[bind(0, 0)]); // 1: pure, binds impure
    p.add(30, 1, &[bind(1, 0)]); // 2: pure, binds tainted
    p.add(40, 1, &[konst(StaticValue::Int(5))]); // 3: independent pure

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
