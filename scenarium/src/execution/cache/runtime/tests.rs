use std::sync::Arc;

use crate::execution::cache::digest::Digest;
use crate::execution::cache::runtime::{RuntimeCache, internals};
use crate::execution::cache::slot::{OutputSnapshot, RuntimeSlot};
use crate::execution::identity::ExecutionNodeId;
use crate::execution::program::index::{NodeColumn, NodeIdx, OutputAddr};
use crate::execution::program::pool::PoolRange;
use crate::execution::program::{ExecutionNode, ExecutionOutput, Program};
use crate::graph::CacheMode;
use crate::node::definition::{FuncBehavior, FuncId};
use crate::node::lambda::OutputDemand;
use crate::{DataType, DynamicValue, RamUsage, StaticValue};

fn out() -> Vec<DynamicValue> {
    vec![DynamicValue::Static(StaticValue::Int(1))]
}

/// Declare one output on `program`, matching the single-value snapshots
/// [`out`] builds.
///
/// `release_dead_outputs` compares a resident snapshot's length against
/// the node's declared port count, so a fixture node declaring none while
/// holding a value is not a shape any real program produces.
fn one_output(program: &mut Program) -> PoolRange<ExecutionOutput> {
    program.outputs.append([ExecutionOutput {
        data_type: DataType::Int,
    }])
}

const DEMANDED: &[OutputDemand] = &[OutputDemand::Produce];

fn complete_snapshot(values: Vec<DynamicValue>) -> OutputSnapshot {
    OutputSnapshot::new(values)
}

/// A slot holding `values`, keyed by `current_digest` and recorded as produced
/// under `produced_under` — the two digests every residency test varies
/// independently, since a hit is exactly the case where they agree.
fn resident_slot(
    current_digest: Option<Digest>,
    produced_under: Option<Digest>,
    values: Vec<DynamicValue>,
) -> RuntimeSlot {
    let mut slot = RuntimeSlot::default();
    slot.current_digest = current_digest;
    slot.load_output(complete_snapshot(values), produced_under);
    slot
}

/// A slot with a digest but no value: a node that has never produced.
fn keyed_slot(current_digest: Option<Digest>) -> RuntimeSlot {
    let mut slot = RuntimeSlot::default();
    slot.current_digest = current_digest;
    slot
}

/// Append a slot at the next dense index. The programs in this file push ids
/// `from_u128(idx + 1)` in the same order, so the cache lands aligned to them
/// without a reconcile.
///
/// It leaves the cache's own alignment unset, so a test that goes on to
/// `reconcile` must build its slots through one instead — that is the pairing
/// `reconcile` reads to carry slots across an install.
fn insert_slot(cache: &mut RuntimeCache, slot: RuntimeSlot) -> NodeIdx {
    let node_idx = NodeIdx(cache.slots.len() as u32);
    cache.slots.push(slot);
    node_idx
}

#[tokio::test]
async fn eviction_clears_only_the_output_cache() {
    let digest = Digest([7u8; 32]);
    let mut slot = resident_slot(Some(digest), Some(digest), out());
    slot.state.set(17_u32);
    slot.event_state.lock().await.set(23_u32);

    let mut cache = RuntimeCache::default();
    let node_idx = insert_slot(&mut cache, slot);
    let e_node_id = ExecutionNodeId::from_u128(1);
    let mut program = Program::default();
    program.push(e_node_id, ExecutionNode::default());
    let failures = cache.evict(&program, &[e_node_id]).await;

    assert!(failures.is_empty());
    assert!(cache.slots[node_idx].output_values().is_none());
    assert_eq!(cache.slots[node_idx].state.get::<u32>(), Some(&17));
    let event_state = cache.slots[node_idx].event_state.lock().await;
    assert_eq!(event_state.get::<u32>(), Some(&23));
}

/// `is_resident_hit` is the resident-cache definition: a slot hits iff it has a
/// current digest, holds values, and those values were produced under that
/// exact digest. The four cases below are the full truth table.
#[test]
fn is_hit_requires_current_digest_values_and_matching_node_digest() {
    let d = Digest([7u8; 32]);
    let other = Digest([8u8; 32]);
    let mut cache = RuntimeCache::default();

    // 0: impure cone (no current digest) — never hits, even holding values.
    let impure = insert_slot(&mut cache, resident_slot(None, Some(d), out()));
    // 1: has a current digest but no cached values.
    let empty = insert_slot(&mut cache, keyed_slot(Some(d)));
    // 2: values present, but produced under a *different* digest (stale).
    let stale = insert_slot(&mut cache, resident_slot(Some(d), Some(other), out()));
    // 3: values produced under the current digest — the only hit.
    let current = insert_slot(&mut cache, resident_slot(Some(d), Some(d), out()));

    assert!(
        !cache.is_resident_hit(impure, DEMANDED),
        "impure cone never hits"
    );
    assert!(
        !cache.is_resident_hit(empty, DEMANDED),
        "no cached values is a miss"
    );
    assert!(
        !cache.is_resident_hit(stale, DEMANDED),
        "values under a stale digest is a miss"
    );
    assert!(
        cache.is_resident_hit(current, DEMANDED),
        "values under the current digest is a hit"
    );
}

#[test]
fn releases_every_resident_value_that_cannot_be_a_future_ram_hit() {
    let current = Digest([7u8; 32]);
    let superseded = Digest([8u8; 32]);
    let cases = [
        (
            "current Ram",
            CacheMode::Ram,
            FuncBehavior::Pure,
            Some(current),
            Some(current),
            true,
        ),
        (
            "current Both",
            CacheMode::Both,
            FuncBehavior::Pure,
            Some(current),
            Some(current),
            true,
        ),
        (
            "impure Ram",
            CacheMode::Ram,
            FuncBehavior::Impure,
            None,
            None,
            false,
        ),
        (
            "newly impure Ram",
            CacheMode::Ram,
            FuncBehavior::Impure,
            Some(current),
            Some(current),
            false,
        ),
        (
            "superseded Both",
            CacheMode::Both,
            FuncBehavior::Pure,
            Some(current),
            Some(superseded),
            false,
        ),
        (
            "current None",
            CacheMode::None,
            FuncBehavior::Pure,
            Some(current),
            Some(current),
            false,
        ),
        (
            "current Disk",
            CacheMode::Disk,
            FuncBehavior::Pure,
            Some(current),
            Some(current),
            false,
        ),
    ];
    let mut cache = RuntimeCache::default();
    let mut program = Program::default();

    for (index, (_, mode, behavior, current_digest, produced_under, _)) in cases.iter().enumerate()
    {
        let e_node_id = ExecutionNodeId::from_u128(index as u128 + 1);
        let outputs = one_output(&mut program);
        program.push(
            e_node_id,
            ExecutionNode {
                cache: *mode,
                behavior: *behavior,
                outputs,
                ..Default::default()
            },
        );
        insert_slot(
            &mut cache,
            resident_slot(*current_digest, *produced_under, out()),
        );
    }

    cache.release_dead_outputs(&program);

    for (index, (name, _, _, _, _, expected_resident)) in cases.iter().enumerate() {
        assert_eq!(
            cache.slots[NodeIdx(index as u32)].output_values().is_some(),
            *expected_resident,
            "{name}"
        );
    }
}

#[test]
fn reconcile_applies_ram_mode_downgrades_without_waiting_for_a_run() {
    let digest = Digest([9u8; 32]);
    let cases = [
        (CacheMode::None, false),
        (CacheMode::Disk, false),
        (CacheMode::Ram, true),
        (CacheMode::Both, true),
    ];
    // One program per install: the values are produced under the first, where
    // every node retains in RAM, and the second is the recompile that downgrades
    // each node to its case's mode.
    let build = |modes: [CacheMode; 4]| {
        let mut program = Program::default();
        for (index, mode) in modes.into_iter().enumerate() {
            let outputs = one_output(&mut program);
            program.push(
                ExecutionNodeId::from_u128(index as u128 + 1),
                ExecutionNode {
                    cache: mode,
                    behavior: FuncBehavior::Pure,
                    outputs,
                    ..Default::default()
                },
            );
        }
        Arc::new(program)
    };

    let mut cache = RuntimeCache::default();
    cache.reconcile(&build([CacheMode::Ram; 4]));
    for (index, _) in cases.iter().enumerate() {
        let slot = &mut cache.slots[NodeIdx(index as u32)];
        slot.current_digest = Some(digest);
        slot.load_output(complete_snapshot(out()), Some(digest));
    }

    cache.reconcile(&build(cases.map(|(mode, _)| mode)));

    for (index, (mode, expected_resident)) in cases.iter().enumerate() {
        assert_eq!(
            cache.slots[NodeIdx(index as u32)].output_values().is_some(),
            *expected_resident,
            "{mode:?}"
        );
    }
}

#[tokio::test]
async fn reconcile_drops_state_only_when_the_owning_implementation_changes() {
    let func_id = FuncId::from_u128(77);
    let e_node_id = ExecutionNodeId::from_u128(1);
    // Each install is its own program — the owner change under test is what a
    // recompile does to the node, not an edit to the program already installed.
    let build = move |func_id, version| {
        let mut program = Program::default();
        let outputs = one_output(&mut program);
        program.push(
            e_node_id,
            ExecutionNode {
                func_id,
                version,
                cache: CacheMode::Ram,
                behavior: FuncBehavior::Pure,
                outputs,
                ..Default::default()
            },
        );
        Arc::new(program)
    };

    let mut cache = RuntimeCache::default();
    cache.reconcile(&build(func_id, 0));
    let digest = Digest([5u8; 32]);
    let node_idx = NodeIdx(0);
    let slot = &mut cache.slots[node_idx];
    slot.state.set(17_u32);
    slot.event_state.lock().await.set(23_u32);
    slot.current_digest = Some(digest);
    slot.load_output(complete_snapshot(out()), Some(digest));

    // Same (func, version): everything survives.
    cache.reconcile(&build(func_id, 0));
    assert_eq!(cache.slots[node_idx].state.get::<u32>(), Some(&17));
    assert_eq!(
        cache.slots[node_idx].event_state.lock().await.get::<u32>(),
        Some(&23),
        "a same-owner reconcile must keep event state"
    );

    // Bumped version: state and event state drop; the resident value stays —
    // its validity is digest-keyed and the digest folds the version.
    cache.reconcile(&build(func_id, 1));
    assert!(
        cache.slots[node_idx].state.is_none(),
        "a version bump must drop the predecessor's state"
    );
    assert!(cache.slots[node_idx].event_state.lock().await.is_none());
    assert!(
        cache.slots[node_idx].output_values().is_some(),
        "reowning must not touch the digest-keyed value"
    );

    // Changed func id at the same version: state drops too.
    cache.slots[node_idx].state.set(31_u32);
    cache.reconcile(&build(FuncId::from_u128(78), 1));
    assert!(
        cache.slots[node_idx].state.is_none(),
        "a func change must drop the predecessor's state"
    );
}

/// Slots follow their stable id when a recompile shifts the dense index space,
/// which is what makes the cache self-describing: it re-pairs against nothing
/// but the ids it already carries.
#[test]
fn reconcile_follows_ids_when_the_index_space_shifts() {
    let build = |ids: &[u128]| {
        let mut program = Program::default();
        for id in ids {
            program.push(
                ExecutionNodeId::from_u128(*id),
                ExecutionNode {
                    cache: CacheMode::Ram,
                    behavior: FuncBehavior::Pure,
                    ..Default::default()
                },
            );
        }
        Arc::new(program)
    };
    let digest = |id: u128| Digest([id as u8; 32]);

    // Ids 1, 2, 3 at indices 0, 1, 2 — each slot stamped with its own digest.
    let mut cache = RuntimeCache::default();
    cache.reconcile(&build(&[1, 2, 3]));
    for i in 0..3u32 {
        let slot = &mut cache.slots[NodeIdx(i)];
        slot.current_digest = Some(digest(i as u128 + 1));
        slot.load_output(complete_snapshot(out()), Some(digest(i as u128 + 1)));
        slot.state.set(i);
    }

    // Node 1 is deleted and node 4 appended: ids sort to 2, 3, 4, so every
    // surviving node slides down one index. Only the new program is named —
    // the one being left is the cache's own.
    cache.reconcile(&build(&[2, 3, 4]));

    assert_eq!(
        cache.slots[NodeIdx(0)].current_digest,
        Some(digest(2)),
        "node 2's slot moved from index 1 to index 0"
    );
    assert_eq!(cache.slots[NodeIdx(0)].state.get::<u32>(), Some(&1));
    assert_eq!(
        cache.slots[NodeIdx(1)].current_digest,
        Some(digest(3)),
        "node 3's slot moved from index 2 to index 1"
    );
    assert_eq!(cache.slots[NodeIdx(1)].state.get::<u32>(), Some(&2));
    assert_eq!(
        cache.slots[NodeIdx(2)].current_digest,
        None,
        "the appended node 4 gets a fresh slot"
    );
    assert!(cache.slots[NodeIdx(2)].output_values().is_none());
    assert_eq!(cache.slots.len(), 3, "the deleted node 1's slot is dropped");
}

#[test]
fn hydrate_turns_a_miss_into_a_hit() {
    let d = Digest([3u8; 32]);
    let mut cache = RuntimeCache::default();
    let node_idx = insert_slot(&mut cache, keyed_slot(Some(d)));
    assert!(
        !cache.is_resident_hit(node_idx, DEMANDED),
        "empty slot misses"
    );

    internals::hydrate(&mut cache, node_idx, complete_snapshot(out()), d);
    assert!(
        cache.is_resident_hit(node_idx, DEMANDED),
        "a slot hydrated under its current digest hits"
    );

    // Hydrating under a digest that is no longer current does not hit.
    cache.slots[node_idx].current_digest = Some(Digest([9u8; 32]));
    assert!(
        !cache.is_resident_hit(node_idx, DEMANDED),
        "current digest moved on ⇒ miss"
    );
}

#[test]
fn resident_hit_derives_coverage_from_values() {
    let digest = Digest([5; 32]);
    let mut cache = RuntimeCache::default();
    let mut slot = keyed_slot(Some(digest));
    slot.invoke_slot(2).outputs[0] = StaticValue::Int(10).into();
    slot.stamp_produced();
    let node_idx = insert_slot(&mut cache, slot);

    let values = cache.slots[node_idx]
        .output_values()
        .expect("the invocation result was stamped resident");
    assert_eq!(values[0].as_i64(), Some(10));
    assert!(matches!(values[1], DynamicValue::Unbound));

    assert!(cache.is_resident_hit(node_idx, &[OutputDemand::Produce, OutputDemand::Skip]));
    assert!(!cache.is_resident_hit(node_idx, &[OutputDemand::Produce, OutputDemand::Produce]));

    cache[node_idx].clear_output_port(0);
    let values = cache.slots[node_idx]
        .output_values()
        .expect("clearing one output keeps the snapshot resident");
    assert!(matches!(
        values,
        [DynamicValue::Unbound, DynamicValue::Unbound]
    ));

    let missing_invocation = std::panic::catch_unwind(|| {
        RuntimeSlot::default().stamp_produced();
    });
    assert!(
        missing_invocation.is_err(),
        "only an invoked resident output can be stamped produced"
    );
}

#[test]
#[cfg(debug_assertions)]
fn debug_assertions_reject_invalid_cache_arities_and_ports() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let snapshot = OutputSnapshot::new(vec![DynamicValue::Unbound]);
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            snapshot.covers_demand(&[OutputDemand::Produce, OutputDemand::Skip]);
        }))
        .is_err(),
        "resident values and output demand require equal arity"
    );

    let e_node_id = ExecutionNodeId::from_u128(1);
    let mut program = Program::default();
    let outputs = program
        .outputs
        .append([ExecutionOutput::default(), ExecutionOutput::default()]);
    program.push(
        e_node_id,
        ExecutionNode {
            outputs,
            ..Default::default()
        },
    );
    let mut cache = RuntimeCache::default();
    insert_slot(
        &mut cache,
        resident_slot(None, None, vec![DynamicValue::Unbound]),
    );
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            cache.read_output_port(
                &program,
                OutputAddr {
                    node_idx: NodeIdx(0),
                    port_idx: 0,
                },
                false,
            );
        }))
        .is_err(),
        "resident values must match the compiled output arity"
    );
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            cache[NodeIdx(0)].clear_output_port(1);
        }))
        .is_err(),
        "a released output port must be in range"
    );
}

#[test]
fn resident_ram_stats_accounts_each_owner_once_and_dedups_the_total() {
    use std::any::Any;
    use std::fmt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::{CustomValue, TypeId};

    #[derive(Debug)]
    struct Payload {
        cpu: usize,
        gpu: usize,
        calls: Arc<AtomicUsize>,
    }
    impl fmt::Display for Payload {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "payload")
        }
    }
    impl CustomValue for Payload {
        fn type_id(&self) -> TypeId {
            TypeId::from_u128(0x5123)
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
            self
        }
        fn ram_bytes(&self) -> RamUsage {
            self.calls.fetch_add(1, Ordering::Relaxed);
            RamUsage {
                cpu: self.cpu,
                gpu: self.gpu,
            }
        }
    }

    let d = Digest([1u8; 32]);
    // One Arc held by two different slots — its bytes exist once.
    let shared_calls = Arc::new(AtomicUsize::new(0));
    let distinct_calls = Arc::new(AtomicUsize::new(0));
    let shared: Arc<dyn CustomValue> = Arc::new(Payload {
        cpu: 100,
        gpu: 10,
        calls: shared_calls.clone(),
    });

    let mut cache = RuntimeCache::default();
    // Slot A: the shared value + a distinct 5/0 value + a scalar (weightless).
    insert_slot(
        &mut cache,
        resident_slot(
            Some(d),
            Some(d),
            vec![
                DynamicValue::Custom(shared.clone()),
                DynamicValue::Custom(Arc::new(Payload {
                    cpu: 5,
                    gpu: 0,
                    calls: distinct_calls.clone(),
                })),
                DynamicValue::Static(StaticValue::Int(9)),
            ],
        ),
    );
    // Slot B: the *same* shared Arc again — must not be counted twice.
    insert_slot(
        &mut cache,
        resident_slot(Some(d), Some(d), vec![DynamicValue::Custom(shared.clone())]),
    );
    // Slot C: empty — contributes zero.
    insert_slot(&mut cache, RuntimeSlot::default());

    // The slots are named by the program they are aligned to — three nodes at
    // ids 1, 2, 3, matching the order `insert_slot` appended them in.
    let mut program = Program::default();
    for index in 0..3u128 {
        program.push(
            ExecutionNodeId::from_u128(index + 1),
            ExecutionNode::default(),
        );
    }

    // shared (100/10) counted once + the 5/0 value; scalar and Empty add nothing.
    let mut by_node = NodeColumn::default();
    let total = cache.resident_ram_stats(&program, &mut by_node);
    assert_eq!(total, RamUsage { cpu: 105, gpu: 10 });
    assert_eq!(total.total(), 115);

    // Per-node: no cross-slot dedup — each node reports what it holds. The column spans
    // the program, so the empty slot C reads zero rather than going unlisted. Slot A holds
    // shared (100/10) + the 5/0 value = 105/10; slot B holds shared again = 100/10.
    assert_eq!(by_node.len(), 3);
    assert_eq!(by_node[NodeIdx(0)], RamUsage { cpu: 105, gpu: 10 });
    assert_eq!(by_node[NodeIdx(1)], RamUsage { cpu: 100, gpu: 10 });
    assert_eq!(by_node[NodeIdx(2)], RamUsage::default());
    assert_eq!(shared_calls.load(Ordering::Relaxed), 2);
    assert_eq!(distinct_calls.load(Ordering::Relaxed), 1);

    let capacity = by_node.capacity();
    let seen_capacity = cache.ram_seen.capacity();
    assert_eq!(
        cache.resident_ram_stats(&program, &mut by_node),
        RamUsage { cpu: 105, gpu: 10 }
    );
    assert_eq!(
        by_node.capacity(),
        capacity,
        "the column is refilled in place"
    );
    assert_eq!(cache.ram_seen.capacity(), seen_capacity);
    assert_eq!(shared_calls.load(Ordering::Relaxed), 4);
    assert_eq!(distinct_calls.load(Ordering::Relaxed), 2);
}
