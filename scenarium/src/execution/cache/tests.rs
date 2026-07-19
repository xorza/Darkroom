use super::*;
use crate::StaticValue;
use crate::node::lambda::OutputDemand;

fn out() -> Vec<DynamicValue> {
    vec![DynamicValue::Static(StaticValue::Int(1))]
}

const DEMANDED: &[OutputDemand] = &[OutputDemand::Produce];

fn complete_snapshot(values: Vec<DynamicValue>) -> OutputSnapshot {
    OutputSnapshot::new(values)
}

fn insert_slot(cache: &mut RuntimeCache, id: u128, slot: RuntimeSlot) -> NodeId {
    let node_id = NodeId::from_u128(id);
    assert!(cache.slots.insert(node_id, slot).is_none());
    node_id
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
        1,
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
        2,
        RuntimeSlot {
            current_digest: Some(d),
            ..Default::default()
        },
    );
    // 2: values present, but produced under a *different* digest (stale).
    let stale = insert_slot(
        &mut cache,
        3,
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
        4,
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
fn hydrate_turns_a_miss_into_a_hit() {
    let d = Digest([3u8; 32]);
    let mut cache = RuntimeCache::default();
    let node_id = insert_slot(
        &mut cache,
        1,
        RuntimeSlot {
            current_digest: Some(d),
            ..Default::default()
        },
    );
    assert!(
        !cache.is_resident_hit(node_id, DEMANDED),
        "empty slot misses"
    );

    test_support::hydrate(&mut cache, node_id, complete_snapshot(out()), d);
    assert!(
        cache.is_resident_hit(node_id, DEMANDED),
        "a slot hydrated under its current digest hits"
    );

    // Hydrating under a digest that is no longer current does not hit.
    cache.slots.get_mut(&node_id).unwrap().current_digest = Some(Digest([9u8; 32]));
    assert!(
        !cache.is_resident_hit(node_id, DEMANDED),
        "current digest moved on ⇒ miss"
    );
}

#[test]
fn replacing_disk_store_clears_only_disk_availability() {
    let mut cache = RuntimeCache::default();
    let on_disk = insert_slot(
        &mut cache,
        1,
        RuntimeSlot {
            value: ValueState::OnDisk {
                coverage: CachedOutputCoverage { ports: vec![true] },
            },
            ..Default::default()
        },
    );
    let resident = insert_slot(
        &mut cache,
        2,
        RuntimeSlot {
            value: ValueState::Resident {
                snapshot: complete_snapshot(out()),
                produced_under: None,
            },
            ..Default::default()
        },
    );

    cache.set_disk_store(DiskStore::default());

    assert!(matches!(cache.slots[&on_disk].value, ValueState::Empty));
    assert_eq!(
        cache.slots[&resident].output_values().unwrap()[0].as_i64(),
        Some(1),
        "resident values do not belong to either disk store"
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
    let node_id = insert_slot(&mut cache, 1, slot);

    let ValueState::Resident { snapshot, .. } = &cache.slots[&node_id].value else {
        panic!("the invocation result was stamped resident");
    };
    assert_eq!(snapshot.values[0].as_i64(), Some(10));
    assert!(matches!(snapshot.values[1], DynamicValue::Unbound));

    assert!(cache.is_resident_hit(node_id, &[OutputDemand::Produce, OutputDemand::Skip]));
    assert!(!cache.is_resident_hit(node_id, &[OutputDemand::Produce, OutputDemand::Produce]));

    cache.clear_output_port(node_id, 0);
    let ValueState::Resident { snapshot, .. } = &cache.slots[&node_id].value else {
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
        1,
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
        2,
        RuntimeSlot {
            current_digest: Some(d),
            value: ValueState::Resident {
                snapshot: complete_snapshot(vec![DynamicValue::Custom(shared.clone())]),
                produced_under: Some(d),
            },
            ..Default::default()
        },
    );
    // Slot C: OnDisk — resident nowhere, contributes zero.
    insert_slot(
        &mut cache,
        3,
        RuntimeSlot {
            current_digest: Some(d),
            value: ValueState::OnDisk {
                coverage: CachedOutputCoverage { ports: vec![true] },
            },
            ..Default::default()
        },
    );

    // shared (100/10) counted once + the 5/0 value; scalar and OnDisk add nothing.
    let stats = cache.resident_ram_stats();
    assert_eq!(stats.total, RamUsage { cpu: 105, gpu: 10 });
    assert_eq!(stats.total.total(), 115);

    // Per-node: no cross-slot dedup — each node reports what it holds. Slot A holds
    // shared (100/10) + the 5/0 value = 105/10; slot B holds shared again = 100/10;
    // the OnDisk slot C is omitted.
    assert_eq!(stats.by_node.len(), 2);
    assert!(stats.by_node.contains(&NodeRamUsage {
        node_id: NodeId::from_u128(1),
        usage: RamUsage { cpu: 105, gpu: 10 },
    }));
    assert!(stats.by_node.contains(&NodeRamUsage {
        node_id: NodeId::from_u128(2),
        usage: RamUsage { cpu: 100, gpu: 10 },
    }));
    assert_eq!(shared_calls.load(Ordering::Relaxed), 2);
    assert_eq!(distinct_calls.load(Ordering::Relaxed), 1);
}
