use crate::graph::identity::FuncId;
use std::sync::Arc;

use super::*;
use crate::StaticValue;
use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::cache::runtime::internals::hydrate;
use crate::execution::cache::slot::OutputSnapshot;
use crate::execution::identity::ExecutionNodeId;
use crate::execution::program::index::{NodeIdx, OutputAddr};
use crate::execution::program::{
    ExecutionBinding, ExecutionInput, ExecutionNode, ExecutionOutput, Program,
};
use crate::graph::node::definition::FuncBehavior;

/// Minimal hand-built `Program` for digest tests. Node ids are
/// `from_u128(idx + 1)`; `bind`'s target id must match that scheme. Output types
/// go straight into the packed output metadata — each output defaults to `Int`,
/// overridable via [`Prog::add_typed`] to exercise the output-signature folding.
#[derive(Debug, Default)]
struct Prog {
    /// Shared like the compile artifact's, so it can be handed to
    /// [`RuntimeCache::reconcile`]; every read of it derefs to a `&Program`.
    program: Arc<Program>,
}

impl Prog {
    /// The program while it is still exclusively this fixture's — every
    /// mutation below goes through here, and it stops being available the
    /// moment a cache is reconciled onto it.
    fn building(&mut self) -> &mut Program {
        Arc::get_mut(&mut self.program).expect("the fixture is built before it is shared")
    }

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
        let pool = self.program.by_id(e_node_id(idx)).inputs.start as usize + input_idx;
        self.building().inputs[pool].stamps_fs_path = true;
    }

    fn add_with(
        &mut self,
        behavior: FuncBehavior,
        func: u128,
        types: &[DataType],
        bindings: &[ExecutionBinding],
    ) -> usize {
        let inputs = self
            .building()
            .inputs
            .append(bindings.iter().map(|binding| ExecutionInput {
                required: false,
                stamps_fs_path: false,
                binding: binding.clone(),
            }));
        let idx = self.program.e_nodes.len();
        let outputs = self.building().outputs.append(
            types
                .iter()
                .cloned()
                .map(|data_type| ExecutionOutput { data_type }),
        );
        let e_node_id = e_node_id(idx);
        self.building().push(
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
    ExecutionBinding::Bind(OutputAddr {
        node_idx: node_idx(idx),
        port_idx: port as u32,
    })
}

fn e_node_id(idx: usize) -> ExecutionNodeId {
    ExecutionNodeId::from_u128(idx as u128 + 1)
}

/// The fixture pushes nodes in index order, so the dense index equals the
/// fixture index.
fn node_idx(idx: usize) -> NodeIdx {
    NodeIdx(idx as u32)
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
/// producers' just-stamped `current_digest` — stopping after `through`. The cache
/// identifies its own paths each call. Returns it, holding every computed digest.
fn digested_cache(program: &Arc<Program>, through: usize) -> RuntimeCache {
    let mut cache = RuntimeCache::default();
    cache.reconcile(program);
    for idx in 0..=through {
        cache.prepare_node_blocking(node_idx(idx));
        cache.stamp_digest(node_idx(idx));
    }
    cache
}

/// One node's content digest, computing only the producer-first prefix it needs.
fn digest_at(program: &Arc<Program>, idx: usize) -> Option<Digest> {
    digested_cache(program, idx)[node_idx(idx)].current_digest
}

/// Every node's content digest, indexed by position.
fn digests(prog: &Prog) -> Vec<Option<Digest>> {
    let last = prog.program.e_nodes.len().saturating_sub(1);
    let cache = digested_cache(&prog.program, last);
    (0..prog.program.e_nodes.len())
        .map(|idx| cache[node_idx(idx)].current_digest)
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

    p.building().by_id_mut(e_node_id(0)).version = 1;
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

    // A path that is not there has no identity to fold at all: the node
    // is left without a digest, and the executor fails it at its turn
    // rather than keying it on an absence.
    std::fs::remove_file(&file).unwrap();
    assert_eq!(
        digest_at(&p.program, 0),
        None,
        "a missing file leaves its node with no digest"
    );

    // The path string is folded on top of the file identity, so two nodes
    // reading equal files under different names still key apart. Planted
    // identities rather than real files: a constant is what makes the
    // encoding pinnable, and no test controls a real file's mtime.
    let planted = |value: StaticValue, path: &str| {
        let mut p = Prog::default();
        p.add(10, 1, &[konst(value)]);
        let mut cache = RuntimeCache::default();
        cache.reconcile(&p.program);
        cache.stamp_file(path, 4, 7);
        cache.node_digest(node_idx(0))
    };
    let here = "definitely-missing-elsewhere";
    let there = "definitely-missing-somewhere";
    let d_here = planted(StaticValue::FsPath(here.into()), here);
    assert_ne!(
        planted(StaticValue::FsPath(there.into()), there),
        d_here,
        "same file identity under a different path ⇒ different digest"
    );
    assert_ne!(
        planted(StaticValue::FsPaths(vec![here.into()]), here),
        d_here,
        "single-path and path-list variants must hash apart"
    );
    // Moves only when the resource-identity encoding changes on purpose —
    // read the new number off the failure and update it here. Every other
    // failure of this line is accidental digest drift, which would
    // silently invalidate every persisted cache blob.
    assert_eq!(
        d_here,
        Some(Digest([
            128, 125, 192, 230, 35, 129, 82, 24, 7, 16, 107, 127, 38, 16, 185, 174, 95, 246, 112,
            199, 104, 177, 218, 96, 191, 140, 142, 182, 57, 112, 38, 97,
        ])),
        "the single-path digest encoding must remain stable"
    );
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
        let producer = cache.node_digest(node_idx(0)).unwrap();
        cache[node_idx(0)].current_digest = Some(producer);
        if let Some(value) = value {
            hydrate(
                &mut cache,
                node_idx(0),
                OutputSnapshot::new(vec![value]),
                producer,
            );
        }
        cache.prepare_node_blocking(node_idx(1));
        cache.prepare_node_blocking(node_idx(2));
        DigestPair {
            typed: cache.node_digest(node_idx(1)),
            plain: cache.node_digest(node_idx(2)),
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
    let DigestPair {
        typed: typed_missing,
        plain: plain_missing,
    } = digests_with(fs_path());
    assert_eq!(
        typed_missing, None,
        "a wired path that is not there has no identity to fold"
    );
    assert_eq!(
        plain_missing, plain_len3,
        "the undeclared consumer never dereferences it, so it folds on regardless"
    );

    // A delivered non-path value folds a distinct marker — still cacheable.
    let typed_int = digests_with(Some(DynamicValue::Static(StaticValue::Int(7)))).typed;
    assert!(
        typed_int.is_some(),
        "a mis-typed wire stays cacheable, unlike an unreadable referent"
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
    let producer = cache.node_digest(node_idx(0)).unwrap();
    cache[node_idx(0)].current_digest = Some(producer);
    hydrate(
        &mut cache,
        node_idx(0),
        OutputSnapshot::new(vec![DynamicValue::Static(StaticValue::FsPath(path))]),
        producer,
    );
    cache[node_idx(0)].current_digest = Some(Digest([9; 32]));
    cache.prepare_node_blocking(node_idx(1));
    assert_eq!(
        cache.node_digest(node_idx(1)),
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

    let d = digests(&p);
    assert_ne!(
        d[1], d[2],
        "consumers reading different ports of one producer must key apart"
    );
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
