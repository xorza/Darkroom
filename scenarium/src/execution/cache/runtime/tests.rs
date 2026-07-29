use std::sync::Arc;

use crate::execution::cache::digest::Digest;
use crate::execution::cache::runtime::{RuntimeCache, internals};
use crate::execution::cache::slot::{OutputSnapshot, RuntimeSlot, ValueState};
use crate::execution::identity::ExecutionNodeId;
use crate::execution::outcome::NodeRamUsage;
use crate::execution::program::index::{NodeIdx, OutputAddr};
use crate::execution::program::pool::PoolRange;
use crate::execution::program::{ExecutionNode, ExecutionOutput, ExecutionProgram};
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
fn one_output(program: &mut ExecutionProgram) -> PoolRange<ExecutionOutput> {
    program.outputs.append([ExecutionOutput {
        data_type: DataType::Int,
    }])
}

const DEMANDED: &[OutputDemand] = &[OutputDemand::Produce];

fn complete_snapshot(values: Vec<DynamicValue>) -> OutputSnapshot {
    OutputSnapshot::new(values)
}

/// Append a slot under the id its dense position implies — `from_u128(idx + 1)`,
/// the same numbering the programs in this file push, so the cache lands aligned
/// to them without a reconcile.
fn insert_slot(cache: &mut RuntimeCache, slot: RuntimeSlot) -> NodeIdx {
    let node_idx = NodeIdx(cache.slots.len() as u32);
    cache
        .e_node_ids
        .push(ExecutionNodeId::from_u128(node_idx.0 as u128 + 1));
    cache.slots.push(slot);
    node_idx
}

#[tokio::test]
async fn eviction_clears_only_the_output_cache() {
    let digest = Digest([7u8; 32]);
    let mut slot = RuntimeSlot {
        current_digest: Some(digest),
        value: ValueState::Resident {
            snapshot: complete_snapshot(out()),
            produced_under: Some(digest),
        },
        ..Default::default()
    };
    slot.state.set(17_u32);
    slot.event_state.lock().await.set(23_u32);

    let mut cache = RuntimeCache::default();
    let node_idx = insert_slot(&mut cache, slot);
    let e_node_id = ExecutionNodeId::from_u128(1);
    let mut program = ExecutionProgram::default();
    program.push(e_node_id, ExecutionNode::default());
    let failures = cache.evict(&program, &[e_node_id]).await;

    assert!(failures.is_empty());
    assert!(matches!(cache.slots[node_idx].value, ValueState::Empty));
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
    let impure = insert_slot(
        &mut cache,
        RuntimeSlot {
            value: ValueState::Resident {
                snapshot: complete_snapshot(out()),
                produced_under: Some(d),
            },
            current_digest: None,
            ..Default::default()
        },
    );
    // 1: has a current digest but no cached values.
    let empty = insert_slot(
        &mut cache,
        RuntimeSlot {
            current_digest: Some(d),
            ..Default::default()
        },
    );
    // 2: values present, but produced under a *different* digest (stale).
    let stale = insert_slot(
        &mut cache,
        RuntimeSlot {
            current_digest: Some(d),
            value: ValueState::Resident {
                snapshot: complete_snapshot(out()),
                produced_under: Some(other),
            },
            ..Default::default()
        },
    );
    // 3: values produced under the current digest — the only hit.
    let current = insert_slot(
        &mut cache,
        RuntimeSlot {
            current_digest: Some(d),
            value: ValueState::Resident {
                snapshot: complete_snapshot(out()),
                produced_under: Some(d),
            },
            ..Default::default()
        },
    );

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
    let mut program = ExecutionProgram::default();

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
            RuntimeSlot {
                current_digest: *current_digest,
                value: ValueState::Resident {
                    snapshot: complete_snapshot(out()),
                    produced_under: *produced_under,
                },
                ..Default::default()
            },
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
    let mut cache = RuntimeCache::default();
    let mut program = ExecutionProgram::default();

    for (index, _) in cases.iter().enumerate() {
        let e_node_id = ExecutionNodeId::from_u128(index as u128 + 1);
        let outputs = one_output(&mut program);
        program.push(
            e_node_id,
            ExecutionNode {
                cache: CacheMode::Ram,
                behavior: FuncBehavior::Pure,
                outputs,
                ..Default::default()
            },
        );
        insert_slot(
            &mut cache,
            RuntimeSlot {
                current_digest: Some(digest),
                value: ValueState::Resident {
                    snapshot: complete_snapshot(out()),
                    produced_under: Some(digest),
                },
                ..Default::default()
            },
        );
    }
    for (index, (mode, _)) in cases.iter().enumerate() {
        program
            .by_id_mut(ExecutionNodeId::from_u128(index as u128 + 1))
            .cache = *mode;
    }

    cache.reconcile(&program);

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
    let mut program = ExecutionProgram::default();
    let outputs = one_output(&mut program);
    let node = move |func_id, version| ExecutionNode {
        func_id,
        version,
        cache: CacheMode::Ram,
        behavior: FuncBehavior::Pure,
        outputs,
        ..Default::default()
    };
    let e_node_id = ExecutionNodeId::from_u128(1);
    program.push(e_node_id, node(func_id, 0));

    let mut cache = RuntimeCache::default();
    cache.reconcile(&program);
    let digest = Digest([5u8; 32]);
    let node_idx = NodeIdx(0);
    let slot = &mut cache.slots[node_idx];
    slot.state.set(17_u32);
    slot.event_state.lock().await.set(23_u32);
    slot.current_digest = Some(digest);
    slot.value = ValueState::Resident {
        snapshot: complete_snapshot(out()),
        produced_under: Some(digest),
    };

    // Same (func, version): everything survives.
    cache.reconcile(&program);
    assert_eq!(cache.slots[node_idx].state.get::<u32>(), Some(&17));
    assert_eq!(
        cache.slots[node_idx].event_state.lock().await.get::<u32>(),
        Some(&23),
        "a same-owner reconcile must keep event state"
    );

    // Bumped version: state and event state drop; the resident value stays —
    // its validity is digest-keyed and the digest folds the version.
    *program.by_id_mut(e_node_id) = node(func_id, 1);
    cache.reconcile(&program);
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
    *program.by_id_mut(e_node_id) = node(FuncId::from_u128(78), 1);
    cache.reconcile(&program);
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
        let mut program = ExecutionProgram::default();
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
        program
    };
    let digest = |id: u128| Digest([id as u8; 32]);

    // Ids 1, 2, 3 at indices 0, 1, 2 — each slot stamped with its own digest.
    let mut cache = RuntimeCache::default();
    cache.reconcile(&build(&[1, 2, 3]));
    for i in 0..3u32 {
        let slot = &mut cache.slots[NodeIdx(i)];
        slot.current_digest = Some(digest(i as u128 + 1));
        slot.value = ValueState::Resident {
            snapshot: complete_snapshot(out()),
            produced_under: Some(digest(i as u128 + 1)),
        };
        slot.state.set(i);
    }

    // Node 1 is deleted and node 4 appended: ids sort to 2, 3, 4, so every
    // surviving node slides down one index.
    cache.reconcile(&build(&[2, 3, 4]));

    assert_eq!(
        cache.e_node_ids.iter().copied().collect::<Vec<_>>(),
        (2..=4).map(ExecutionNodeId::from_u128).collect::<Vec<_>>(),
        "the cache tracks the new index order"
    );
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
    let node_idx = insert_slot(
        &mut cache,
        RuntimeSlot {
            current_digest: Some(d),
            ..Default::default()
        },
    );
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
    let mut slot = RuntimeSlot {
        current_digest: Some(digest),
        ..Default::default()
    };
    slot.invoke_slot(2).outputs[0] = StaticValue::Int(10).into();
    slot.stamp_produced();
    let node_idx = insert_slot(&mut cache, slot);

    let ValueState::Resident { snapshot, .. } = &cache.slots[node_idx].value else {
        panic!("the invocation result was stamped resident");
    };
    assert_eq!(snapshot.values[0].as_i64(), Some(10));
    assert!(matches!(snapshot.values[1], DynamicValue::Unbound));

    assert!(cache.is_resident_hit(node_idx, &[OutputDemand::Produce, OutputDemand::Skip]));
    assert!(!cache.is_resident_hit(node_idx, &[OutputDemand::Produce, OutputDemand::Produce]));

    cache.clear_output_port(OutputAddr {
        node_idx,
        port_idx: 0,
    });
    let ValueState::Resident { snapshot, .. } = &cache.slots[node_idx].value else {
        panic!("clearing one output keeps the snapshot resident");
    };
    assert!(matches!(
        snapshot.values.as_slice(),
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
    let mut program = ExecutionProgram::default();
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
        RuntimeSlot {
            value: ValueState::Resident {
                snapshot: OutputSnapshot::new(vec![DynamicValue::Unbound]),
                produced_under: None,
            },
            ..Default::default()
        },
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
            cache.clear_output_port(OutputAddr {
                node_idx: NodeIdx(0),
                port_idx: 1,
            });
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
        RuntimeSlot {
            current_digest: Some(d),
            value: ValueState::Resident {
                snapshot: complete_snapshot(vec![
                    DynamicValue::Custom(shared.clone()),
                    DynamicValue::Custom(Arc::new(Payload {
                        cpu: 5,
                        gpu: 0,
                        calls: distinct_calls.clone(),
                    })),
                    DynamicValue::Static(StaticValue::Int(9)),
                ]),
                produced_under: Some(d),
            },
            ..Default::default()
        },
    );
    // Slot B: the *same* shared Arc again — must not be counted twice.
    insert_slot(
        &mut cache,
        RuntimeSlot {
            current_digest: Some(d),
            value: ValueState::Resident {
                snapshot: complete_snapshot(vec![DynamicValue::Custom(shared.clone())]),
                produced_under: Some(d),
            },
            ..Default::default()
        },
    );
    // Slot C: empty — contributes zero.
    insert_slot(&mut cache, RuntimeSlot::default());

    // shared (100/10) counted once + the 5/0 value; scalar and Empty add nothing.
    let mut by_node = Vec::new();
    let total = cache.resident_ram_stats(&mut by_node);
    assert_eq!(total, RamUsage { cpu: 105, gpu: 10 });
    assert_eq!(total.total(), 115);

    // Per-node: no cross-slot dedup — each node reports what it holds. Slot A holds
    // shared (100/10) + the 5/0 value = 105/10; slot B holds shared again = 100/10;
    // the empty slot C is omitted.
    assert_eq!(by_node.len(), 2);
    assert!(by_node.contains(&NodeRamUsage {
        e_node_id: ExecutionNodeId::from_u128(1),
        usage: RamUsage { cpu: 105, gpu: 10 },
    }));
    assert!(by_node.contains(&NodeRamUsage {
        e_node_id: ExecutionNodeId::from_u128(2),
        usage: RamUsage { cpu: 100, gpu: 10 },
    }));
    assert_eq!(shared_calls.load(Ordering::Relaxed), 2);
    assert_eq!(distinct_calls.load(Ordering::Relaxed), 1);

    let allocation = by_node.as_ptr();
    let capacity = by_node.capacity();
    let seen_capacity = cache.ram_seen.capacity();
    assert_eq!(
        cache.resident_ram_stats(&mut by_node),
        RamUsage { cpu: 105, gpu: 10 }
    );
    assert_eq!(by_node.as_ptr(), allocation);
    assert_eq!(by_node.capacity(), capacity);
    assert_eq!(cache.ram_seen.capacity(), seen_capacity);
    assert_eq!(shared_calls.load(Ordering::Relaxed), 4);
    assert_eq!(distinct_calls.load(Ordering::Relaxed), 2);
}
