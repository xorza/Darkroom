use super::*;
use common::TempFile;

use crate::ConstValue;
use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::cache::slot::OutputSnapshot;
use crate::execution::compile::compiled_graph::ExecutionBinding;
use crate::execution::identity::{NodeIdx, OutputAddr};
use crate::graph::identity::FuncId;
use crate::testing::program::node_builder::NodeBuilder;
use crate::testing::program::{Placed, ProgramBuilder};

/// A content-cacheable node of func `func`, declaring `count` `Int` outputs.
///
/// `Int` rather than the builder's polymorphic default because the output
/// signature folds into the digest, and one case below pins the resulting bytes.
fn pure(prog: &mut ProgramBuilder, func: u128, count: usize) -> NodeBuilder<'_> {
    typed(prog, func, &vec![DataType::Int; count])
}

/// [`pure`] with explicit output types, for the signature-folding cases.
fn typed<'a>(prog: &'a mut ProgramBuilder, func: u128, types: &[DataType]) -> NodeBuilder<'a> {
    prog.node()
        .pure()
        .func(FuncId::from_u128(func))
        .output_types(types.iter().cloned())
}

/// Every node's content digest, folded producer-first the way the executor does
/// — each node reading its producers' just-stamped `current_digest`, over a
/// cache that identifies its own filesystem paths afresh.
#[derive(Debug)]
struct Digests(RuntimeCache);

impl Digests {
    fn of(prog: &ProgramBuilder) -> Self {
        let program = prog.program();
        let mut cache = RuntimeCache::default();
        cache.install_for_test(program);
        for idx in 0..program.e_nodes.len() {
            let node_idx = NodeIdx(idx as u32);
            cache.prepare_node_blocking(program, node_idx);
            cache.stamp_digest(program, node_idx);
        }
        Self(cache)
    }

    fn at(&self, node: Placed) -> Option<Digest> {
        self.0[node.node_idx].current_digest
    }
}

#[derive(Debug)]
struct DigestPair {
    typed: Option<Digest>,
    plain: Option<Digest>,
}

#[test]
fn deterministic_and_per_function_distinct() {
    // A → B (B binds A.0), plus an independent C with a const input.
    let mut p = ProgramBuilder::default();
    let a = pure(&mut p, 10, 1).add();
    let b = pure(&mut p, 20, 1).input(a.out(0)).add();
    let c = pure(&mut p, 30, 1).const_input(ConstValue::Int(5)).add();

    let first = Digests::of(&p);
    let second = Digests::of(&p); // same engine inputs → identical digests
    for node in [a, b, c] {
        assert_eq!(
            first.at(node),
            second.at(node),
            "digest must be deterministic"
        );
    }

    // Distinct functions ⇒ distinct digests (no accidental collisions).
    assert_ne!(first.at(a), first.at(b));
    assert_ne!(first.at(b), first.at(c));
    assert_ne!(first.at(a), first.at(c));

    p.program_mut().by_id_mut(a.node_id).func_id = FuncId::from_u128(11);
    let refunced = Digests::of(&p);
    assert_ne!(
        first.at(a),
        refunced.at(a),
        "a function identity re-keys its node"
    );
    assert_ne!(
        first.at(b),
        refunced.at(b),
        "a function identity propagates downstream"
    );
    assert_eq!(
        first.at(c),
        refunced.at(c),
        "an independent node ignores another function's identity"
    );
}

#[test]
fn const_change_propagates_downstream_only() {
    let build = |a_const: i64| {
        let mut p = ProgramBuilder::default();
        let a = pure(&mut p, 10, 1)
            .const_input(ConstValue::Int(a_const))
            .add();
        let b = pure(&mut p, 20, 1).input(a.out(0)).add();
        let c = pure(&mut p, 30, 1).const_input(ConstValue::Int(9)).add();
        (Digests::of(&p), a, b, c)
    };
    let (base, a, b, c) = build(1);
    let (changed, ..) = build(2);

    assert_ne!(base.at(a), changed.at(a), "A's own digest tracks its const");
    assert_ne!(
        base.at(b),
        changed.at(b),
        "B downstream of A must change too"
    );
    assert_eq!(base.at(c), changed.at(c), "independent C is unaffected");
}

#[test]
fn structurally_identical_nodes_share_digest() {
    // Two nodes, same func, same (input-identical) bindings ⇒ equal
    // node digests — the property that lets the store dedup repeated work.
    let mut p = ProgramBuilder::default();
    let first = pure(&mut p, 10, 1).const_input(ConstValue::Int(7)).add();
    let second = pure(&mut p, 10, 1).const_input(ConstValue::Int(7)).add();
    let d = Digests::of(&p);
    assert_eq!(d.at(first), d.at(second));

    // Differ in func or const ⇒ digests diverge.
    let mut q = ProgramBuilder::default();
    let base = pure(&mut q, 10, 1).const_input(ConstValue::Int(7)).add();
    let other_func = pure(&mut q, 11, 1).const_input(ConstValue::Int(7)).add();
    let other_const = pure(&mut q, 10, 1).const_input(ConstValue::Int(8)).add();
    let dq = Digests::of(&q);
    assert_ne!(dq.at(base), dq.at(other_func));
    assert_ne!(dq.at(base), dq.at(other_const));
}

#[test]
fn fs_path_folds_file_identity_and_path() {
    // An `FsPath` const folds its resolved file identity (len, mtime — see
    // `fs_file_id`) on top of the path string, so a file change re-keys: this is
    // what stops machine B serving A's result for B's files. The resolver is the
    // real filesystem, so exercise it with a temp file. `Digests::of` re-stats it
    // on each call (a fresh engine).
    let selected = TempFile::with_extension("scenarium-digest-fs-path", "bin");
    let file = selected.path();
    let path = selected.to_str();
    let path_node = |paths: ConstValue| {
        let mut p = ProgramBuilder::default();
        let node = pure(&mut p, 10, 1).const_input(paths).add();
        (p, node)
    };

    let (p, single) = path_node(ConstValue::FsPath(path.clone()));
    std::fs::write(file, b"x").unwrap(); // len 1
    let d_len1 = Digests::of(&p).at(single);
    std::fs::write(file, b"xyz").unwrap(); // len 3 — file identity changed
    let d_len3 = Digests::of(&p).at(single);
    assert_ne!(
        d_len1, d_len3,
        "a file content change must re-key the digest"
    );

    let unselected = TempFile::with_extension("scenarium-digest-unselected", "bin");
    std::fs::write(unselected.path(), b"not selected").unwrap();
    assert_eq!(
        Digests::of(&p).at(single),
        d_len3,
        "an unselected sibling file must not affect a single-path digest"
    );

    let second = TempFile::with_extension("scenarium-digest-second", "bin");
    std::fs::write(second.path(), b"second").unwrap();
    let second_path = second.to_str();
    let (two_file_prog, both) =
        path_node(ConstValue::FsPaths(vec![path.clone(), second_path.clone()]));
    let two_files = Digests::of(&two_file_prog).at(both);
    std::fs::write(second.path(), b"second changed").unwrap();
    let second_edited = Digests::of(&two_file_prog).at(both);
    assert_ne!(
        two_files, second_edited,
        "editing any selected file must re-key the list"
    );

    let (reversed, swapped) = path_node(ConstValue::FsPaths(vec![second_path, path.clone()]));
    assert_ne!(
        Digests::of(&reversed).at(swapped),
        second_edited,
        "path-list order is part of the authored input"
    );

    // A path that is not there has no identity to fold at all: the node
    // is left without a digest, and the executor fails it at its turn
    // rather than keying it on an absence.
    std::fs::remove_file(file).unwrap();
    assert_eq!(
        Digests::of(&p).at(single),
        None,
        "a missing file leaves its node with no digest"
    );

    // An *unset* path is not a missing one. It names nothing to stat, so the
    // node keeps a sound key and runs — rather than being failed at stamp time
    // over an `ENOENT` on the empty string, every run, until a file is picked.
    let unset = |value: ConstValue| {
        let (p, node) = path_node(value);
        Digests::of(&p).at(node)
    };
    let d_empty = unset(ConstValue::FsPath(String::new()));
    assert!(d_empty.is_some(), "an unset path leaves its node cacheable");
    let d_blank = unset(ConstValue::FsPath("  ".into()));
    assert!(d_blank.is_some(), "whitespace names no file either");
    assert_ne!(
        d_blank, d_empty,
        "the authored string still folds — a blank path is not the empty one"
    );
    assert!(
        unset(ConstValue::FsPaths(vec![String::new()])).is_some(),
        "an unset slot inside a list is unset too"
    );

    // One unset port must not cost the *run* its batched stamping: before the
    // walk skipped it, `metadata("")` failed the whole shared pass, and which
    // of the other nodes had already landed was `HashSet` drain order.
    std::fs::write(file, b"x").unwrap();
    let mut batch = ProgramBuilder::default();
    let blank_node = pure(&mut batch, 11, 1)
        .const_input(ConstValue::FsPath(String::new()))
        .add();
    let real_node = pure(&mut batch, 12, 1)
        .const_input(ConstValue::FsPath(path.clone()))
        .add();
    let batched = batch.program();
    let mut cache = RuntimeCache::default();
    cache.install_for_test(batched);
    cache
        .prepare_nodes_blocking(batched, [blank_node.node_idx, real_node.node_idx])
        .expect("an unset path is not a walk failure");
    assert!(
        cache.node_digest(batched, real_node.node_idx).is_some(),
        "a named path still stamps when batched beside an unset one"
    );
    std::fs::remove_file(file).unwrap();

    // The path string is folded on top of the file identity, so two nodes
    // reading equal files under different names still key apart. Planted
    // identities rather than real files: a constant is what makes the
    // encoding pinnable, and no test controls a real file's mtime.
    let planted = |value: ConstValue, path: &str| {
        let (p, node) = path_node(value);
        let mut cache = RuntimeCache::default();
        cache.install_for_test(p.program());
        cache.stamp_file(path, 4, 7);
        cache.node_digest(p.program(), node.node_idx)
    };
    let here = "definitely-missing-elsewhere";
    let there = "definitely-missing-somewhere";
    let d_here = planted(ConstValue::FsPath(here.into()), here);
    assert_ne!(
        planted(ConstValue::FsPath(there.into()), there),
        d_here,
        "same file identity under a different path ⇒ different digest"
    );
    assert_ne!(
        planted(ConstValue::FsPaths(vec![here.into()]), here),
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
            20, 55, 162, 139, 252, 245, 8, 67, 80, 113, 23, 85, 78, 225, 77, 88, 157, 24, 172, 160,
            26, 161, 188, 173, 231, 38, 251, 148, 76, 195, 254, 125,
        ])),
        "the single-path digest encoding must remain stable"
    );
}

/// A **Bind-delivered** path re-keys its consumer like a const one: the fold reads the
/// producer's delivered value and stats the pointed-at file live — but only through an
/// `FsPath`-declared input, and only once the value is readable (unreadable ⇒ `None`,
/// the taint the run loop's reach-time re-stamp resolves).
#[test]
fn bound_fs_path_folds_delivered_file_identity() {
    use crate::DynamicValue;

    let delivered = TempFile::with_extension("scenarium-digest-bound-fs", "bin");
    let file = delivered.path();
    let path = delivered.to_str();

    // A producer, a consumer whose input is `FsPath`-declared, and a control
    // consumer reading the same port through an undeclared input — no fold.
    let mut p = ProgramBuilder::default();
    let producer = pure(&mut p, 10, 1).add();
    let declared = pure(&mut p, 20, 1).fs_path_input(producer.out(0)).add();
    let control = pure(&mut p, 20, 1).input(producer.out(0)).add();

    // Stamp the producer and install `value` as its delivered output (`None` leaves the
    // slot empty — an unreadable value), then fold both consumers.
    let digests_with = |value: Option<DynamicValue>| {
        let program = p.program();
        let mut cache = RuntimeCache::default();
        cache.install_for_test(program);
        let stamped = cache.node_digest(program, producer.node_idx).unwrap();
        cache[producer.node_idx].current_digest = Some(stamped);
        if let Some(value) = value {
            cache.hydrate(producer.node_idx, OutputSnapshot::new(vec![value]), stamped);
        }
        cache.prepare_node_blocking(program, declared.node_idx);
        cache.prepare_node_blocking(program, control.node_idx);
        DigestPair {
            typed: cache.node_digest(program, declared.node_idx),
            plain: cache.node_digest(program, control.node_idx),
        }
    };
    let fs_path = || Some(DynamicValue::Static(ConstValue::FsPath(path.clone())));

    std::fs::write(file, b"x").unwrap(); // len 1
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

    std::fs::write(file, b"xyz").unwrap(); // len 3 — the file identity changed
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

    let second = TempFile::with_extension("scenarium-digest-bound-fs-second", "bin");
    std::fs::write(second.path(), b"second").unwrap();
    let fs_paths = || {
        Some(DynamicValue::Static(ConstValue::FsPaths(vec![
            path.clone(),
            second.to_str(),
        ])))
    };
    let typed_list = digests_with(fs_paths()).typed;
    std::fs::write(second.path(), b"second changed").unwrap();
    assert_ne!(
        digests_with(fs_paths()).typed,
        typed_list,
        "a wired path list re-keys when any selected file changes"
    );
    std::fs::remove_file(second.path()).unwrap();

    // An unset slot keys on its *position*, which is what the marker buys over
    // skipping the slot: a Bind fold writes no authored strings alongside the
    // identities, so a skip would leave `["", p]` and `[p, ""]` making the same
    // two writes in the same order. Reaching that in a real run also takes a
    // `Pure` producer that isn't — an honest one cannot deliver two values
    // under the one digest folded just above — which is why this plants the
    // pair rather than producing it. The marker is what keeps `hash_fs_paths`
    // decodable from its own arguments instead of from that caller invariant.
    let with_blank = |paths: Vec<String>| {
        digests_with(Some(DynamicValue::Static(ConstValue::FsPaths(paths)))).typed
    };
    let blank_first = with_blank(vec![String::new(), path.clone()]);
    let blank_last = with_blank(vec![path.clone(), String::new()]);
    assert!(
        blank_first.is_some(),
        "an unset slot keeps the declared consumer cacheable"
    );
    assert_ne!(
        blank_first, blank_last,
        "where the unset slot sits is part of the key"
    );

    std::fs::remove_file(file).unwrap();
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
    let typed_int = digests_with(Some(DynamicValue::Static(ConstValue::Int(7)))).typed;
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

    let program = p.program();
    let mut cache = RuntimeCache::default();
    cache.install_for_test(program);
    let stamped = cache.node_digest(program, producer.node_idx).unwrap();
    cache[producer.node_idx].current_digest = Some(stamped);
    cache.hydrate(
        producer.node_idx,
        OutputSnapshot::new(vec![DynamicValue::Static(ConstValue::FsPath(path))]),
        stamped,
    );
    cache[producer.node_idx].current_digest = Some(Digest([9; 32]));
    cache.prepare_node_blocking(program, declared.node_idx);
    assert_eq!(
        cache.node_digest(program, declared.node_idx),
        None,
        "a path value produced under an old producer digest is unreadable"
    );
}

#[test]
fn output_ports_are_disambiguated() {
    // One producer with two outputs; consumers on different ports differ.
    let mut p = ProgramBuilder::default();
    let a = pure(&mut p, 10, 2).add();
    let on_first = pure(&mut p, 20, 1).input(a.out(0)).add();
    let on_second = pure(&mut p, 20, 1).input(a.out(1)).add();

    let d = Digests::of(&p);
    assert_ne!(
        d.at(on_first),
        d.at(on_second),
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

    // One func declaring two different signatures: the digests must diverge on
    // the type, on the arity, and on a custom type's identity.
    let signature_pair = |left: &[DataType], right: &[DataType]| {
        let mut p = ProgramBuilder::default();
        let a = typed(&mut p, 10, left).add();
        let b = typed(&mut p, 10, right).add();
        let d = Digests::of(&p);
        (d.at(a), d.at(b))
    };
    let (int_out, float_out) = signature_pair(&[DataType::Int], &[DataType::Float]);
    assert_ne!(int_out, float_out, "a flipped output type re-keys the node");

    let (one_port, two_ports) = signature_pair(&[DataType::Int], &[DataType::Int, DataType::Int]);
    assert_ne!(one_port, two_ports, "an added output port re-keys the node");

    let (custom_one, custom_two) = signature_pair(
        &[DataType::Custom(TypeId::from_u128(1))],
        &[DataType::Custom(TypeId::from_u128(2))],
    );
    assert_ne!(
        custom_one, custom_two,
        "distinct custom output types re-key"
    );

    // A producer's output-type change propagates to its consumer downstream.
    let build = |producer_type: DataType| {
        let mut p = ProgramBuilder::default();
        let producer = typed(&mut p, 10, &[producer_type]).add();
        let consumer = typed(&mut p, 20, &[DataType::Int])
            .input(producer.out(0))
            .add();
        let d = Digests::of(&p);
        (d.at(producer), d.at(consumer))
    };
    let (base_producer, base_consumer) = build(DataType::Int);
    let (changed_producer, changed_consumer) = build(DataType::Float);
    assert_ne!(
        base_producer, changed_producer,
        "producer's digest tracks its output type"
    );
    assert_ne!(
        base_consumer, changed_consumer,
        "and the change propagates to the consumer"
    );
}

#[test]
fn cycle_yields_none() {
    // A binds B.0, B binds A.0 — a malformed program (the planner rejects it
    // separately); the digest pass must break the recursion, not loop.
    //
    // The forward reference is spelled as a raw address, since the node it names
    // does not exist until the line after.
    let mut p = ProgramBuilder::default();
    let forward = ExecutionBinding::Bind(OutputAddr {
        node_idx: NodeIdx(1),
        port_idx: 0,
    });
    let a = pure(&mut p, 10, 1).input(forward).add();
    pure(&mut p, 20, 1).input(a.out(0)).add();

    assert_eq!(Digests::of(&p).at(a), None);
}

#[test]
fn impure_node_and_its_dependents_are_none() {
    // src (impure) → mid (pure) → sink (pure). The impure source taints the
    // whole downstream chain; an independent pure node stays `Some`.
    let mut p = ProgramBuilder::default();
    let src = p
        .node()
        .func(FuncId::from_u128(10))
        .output_types([DataType::Int])
        .add();
    let mid = pure(&mut p, 20, 1).input(src.out(0)).add();
    let sink = pure(&mut p, 30, 1).input(mid.out(0)).add();
    let independent = pure(&mut p, 40, 1).const_input(ConstValue::Int(5)).add();

    let d = Digests::of(&p);
    assert_eq!(d.at(src), None, "impure node ⇒ None");
    assert_eq!(d.at(mid), None, "pure node under impure ⇒ None");
    assert_eq!(d.at(sink), None, "taint flows the whole way up");
    assert!(
        d.at(independent).is_some(),
        "independent pure node is unaffected"
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
