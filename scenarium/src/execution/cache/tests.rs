use super::*;
use crate::data::StaticValue;

fn out() -> Vec<DynamicValue> {
    vec![DynamicValue::Static(StaticValue::Int(1))]
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
    cache.slots.add(RuntimeSlot {
        id: NodeId::from_u128(1),
        value: ValueState::Resident {
            values: out(),
            produced_under: Some(d),
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
        },
        ..Default::default()
    });

    assert!(!cache.is_resident_hit(NodeIdx(0)), "impure cone never hits");
    assert!(
        !cache.is_resident_hit(NodeIdx(1)),
        "no cached values is a miss"
    );
    assert!(
        !cache.is_resident_hit(NodeIdx(2)),
        "values under a stale digest is a miss"
    );
    assert!(
        cache.is_resident_hit(NodeIdx(3)),
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
    assert!(!cache.is_resident_hit(NodeIdx(0)), "empty slot misses");

    cache.hydrate(NodeIdx(0), out(), d);
    assert!(
        cache.is_resident_hit(NodeIdx(0)),
        "a slot hydrated under its current digest hits"
    );

    // Hydrating under a digest that is no longer current does not hit.
    cache.slots[0].current_digest = Some(Digest([9u8; 32]));
    assert!(
        !cache.is_resident_hit(NodeIdx(0)),
        "current digest moved on ⇒ miss"
    );
}

#[test]
fn resident_ram_usage_sums_custom_values_and_dedups_shared_arcs() {
    use std::any::Any;
    use std::fmt;

    use crate::data::{CustomValue, TypeId};

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
        },
        ..Default::default()
    });
    // Slot C: OnDisk — resident nowhere, contributes zero.
    cache.slots.add(RuntimeSlot {
        id: NodeId::from_u128(3),
        current_digest: Some(d),
        value: ValueState::OnDisk,
        ..Default::default()
    });

    // shared (100/10) counted once + the 5/0 value; scalar and OnDisk add nothing.
    let usage = cache.resident_ram_usage();
    assert_eq!(usage, RamUsage { cpu: 105, gpu: 10 });
    assert_eq!(usage.total(), 115);
}
