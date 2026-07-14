use super::*;
use crate::StaticValue;
use crate::node::lambda::OutputUsage;

fn out() -> Vec<DynamicValue> {
    vec![DynamicValue::Static(StaticValue::Int(1))]
}

const NEEDED: &[OutputUsage] = &[OutputUsage::Needed(1)];

/// `is_resident_hit` is the resident-cache definition: a slot hits iff it has a
/// current digest, holds values, and those values were produced under that
/// exact digest. The four cases below are the full truth table.
#[test]
fn is_hit_requires_current_digest_values_and_matching_node_digest() {
    let d = Digest([7u8; 32]);
    let other = Digest([8u8; 32]);
    let mut cache = RuntimeCache::default();

    // 0: impure cone (no current digest) — never hits, even holding values.
    cache.slots.add(RuntimeSlot {
        id: NodeId::from_u128(1),
        value: ValueState::Resident {
            values: out(),
            produced_under: Some(d),
            materialized: MaterializedOutputs::all(1),
        },
        current_digest: None,
        ..Default::default()
    });
    // 1: has a current digest but no cached values.
    cache.slots.add(RuntimeSlot {
        id: NodeId::from_u128(2),
        current_digest: Some(d),
        ..Default::default()
    });
    // 2: values present, but produced under a *different* digest (stale).
    cache.slots.add(RuntimeSlot {
        id: NodeId::from_u128(3),
        current_digest: Some(d),
        value: ValueState::Resident {
            values: out(),
            produced_under: Some(other),
            materialized: MaterializedOutputs::all(1),
        },
        ..Default::default()
    });
    // 3: values produced under the current digest — the only hit.
    cache.slots.add(RuntimeSlot {
        id: NodeId::from_u128(4),
        current_digest: Some(d),
        value: ValueState::Resident {
            values: out(),
            produced_under: Some(d),
            materialized: MaterializedOutputs::all(1),
        },
        ..Default::default()
    });

    assert!(
        !cache.is_resident_hit(NodeIdx(0), NEEDED),
        "impure cone never hits"
    );
    assert!(
        !cache.is_resident_hit(NodeIdx(1), NEEDED),
        "no cached values is a miss"
    );
    assert!(
        !cache.is_resident_hit(NodeIdx(2), NEEDED),
        "values under a stale digest is a miss"
    );
    assert!(
        cache.is_resident_hit(NodeIdx(3), NEEDED),
        "values under the current digest is a hit"
    );
}

#[test]
fn hydrate_turns_a_miss_into_a_hit() {
    let d = Digest([3u8; 32]);
    let mut cache = RuntimeCache::default();
    cache.slots.add(RuntimeSlot {
        id: NodeId::from_u128(1),
        current_digest: Some(d),
        ..Default::default()
    });
    assert!(
        !cache.is_resident_hit(NodeIdx(0), NEEDED),
        "empty slot misses"
    );

    cache.hydrate(NodeIdx(0), out(), d, MaterializedOutputs::all(1));
    assert!(
        cache.is_resident_hit(NodeIdx(0), NEEDED),
        "a slot hydrated under its current digest hits"
    );

    // Hydrating under a digest that is no longer current does not hit.
    cache.slots[0].current_digest = Some(Digest([9u8; 32]));
    assert!(
        !cache.is_resident_hit(NodeIdx(0), NEEDED),
        "current digest moved on ⇒ miss"
    );
}

#[test]
fn resident_hit_requires_every_demanded_output_to_be_materialized() {
    let digest = Digest([5; 32]);
    let mut cache = RuntimeCache::default();
    cache.slots.add(RuntimeSlot {
        id: NodeId::from_u128(1),
        current_digest: Some(digest),
        value: ValueState::Resident {
            values: vec![StaticValue::Int(10).into(), DynamicValue::Unbound],
            produced_under: Some(digest),
            materialized: MaterializedOutputs::from_bytes(&[1, 0]).unwrap(),
        },
        ..Default::default()
    });

    assert!(cache.is_resident_hit(NodeIdx(0), &[OutputUsage::Needed(1), OutputUsage::Skip]));
    assert!(!cache.is_resident_hit(
        NodeIdx(0),
        &[OutputUsage::Needed(1), OutputUsage::Needed(1)]
    ));
}

#[test]
fn resident_ram_usage_sums_custom_values_and_dedups_shared_arcs() {
    use std::any::Any;
    use std::fmt;

    use crate::{CustomValue, TypeId};

    #[derive(Debug)]
    struct Payload {
        cpu: usize,
        gpu: usize,
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
            RamUsage {
                cpu: self.cpu,
                gpu: self.gpu,
            }
        }
    }

    let d = Digest([1u8; 32]);
    // One Arc held by two different slots — its bytes exist once.
    let shared: Arc<dyn CustomValue> = Arc::new(Payload { cpu: 100, gpu: 10 });

    let mut cache = RuntimeCache::default();
    // Slot A: the shared value + a distinct 5/0 value + a scalar (weightless).
    cache.slots.add(RuntimeSlot {
        id: NodeId::from_u128(1),
        current_digest: Some(d),
        value: ValueState::Resident {
            values: vec![
                DynamicValue::Custom(shared.clone()),
                DynamicValue::Custom(Arc::new(Payload { cpu: 5, gpu: 0 })),
                DynamicValue::Static(StaticValue::Int(9)),
            ],
            produced_under: Some(d),
            materialized: MaterializedOutputs::all(3),
        },
        ..Default::default()
    });
    // Slot B: the *same* shared Arc again — must not be counted twice.
    cache.slots.add(RuntimeSlot {
        id: NodeId::from_u128(2),
        current_digest: Some(d),
        value: ValueState::Resident {
            values: vec![DynamicValue::Custom(shared.clone())],
            produced_under: Some(d),
            materialized: MaterializedOutputs::all(1),
        },
        ..Default::default()
    });
    // Slot C: OnDisk — resident nowhere, contributes zero.
    cache.slots.add(RuntimeSlot {
        id: NodeId::from_u128(3),
        current_digest: Some(d),
        value: ValueState::OnDisk {
            materialized: MaterializedOutputs::all(1),
        },
        ..Default::default()
    });

    // shared (100/10) counted once + the 5/0 value; scalar and OnDisk add nothing.
    let usage = cache.resident_ram_usage();
    assert_eq!(usage, RamUsage { cpu: 105, gpu: 10 });
    assert_eq!(usage.total(), 115);

    // Per-node: no cross-slot dedup — each node reports what it holds. Slot A holds
    // shared (100/10) + the 5/0 value = 105/10; slot B holds shared again = 100/10;
    // the OnDisk slot C is omitted.
    let by_node = cache.resident_ram_by_node();
    assert_eq!(
        by_node,
        vec![
            NodeRamUsage {
                node_id: NodeId::from_u128(1),
                usage: RamUsage { cpu: 105, gpu: 10 },
            },
            NodeRamUsage {
                node_id: NodeId::from_u128(2),
                usage: RamUsage { cpu: 100, gpu: 10 },
            },
        ]
    );
}
