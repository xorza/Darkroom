use super::*;

/// A top-level node (empty descent path) keeps its own id verbatim — this is
/// what lets a func-only graph map to itself and per-node caches survive an
/// edit. (Full flattening of composites is covered by the integration suite.)
#[test]
fn flatten_id_is_identity_at_top_level() {
    let id = NodeId::from_u128(0xABCD);
    assert_eq!(flatten_id(&[], id), id);
}

/// A nested node's id is a deterministic hash of (descent path, interior id):
/// stable across calls, distinct from the bare interior id, and sensitive to
/// both the path and the interior id (so two instances of one composite, or
/// two interior nodes, never collide).
#[test]
fn flatten_id_nested_is_deterministic_and_path_sensitive() {
    let interior = NodeId::from_u128(7);
    let path = [NodeId::from_u128(1), NodeId::from_u128(2)];

    let id = flatten_id(&path, interior);
    assert_eq!(id, flatten_id(&path, interior), "deterministic");
    assert_ne!(
        id, interior,
        "a nested id is remapped, not the bare interior id"
    );

    let other_path = [NodeId::from_u128(1), NodeId::from_u128(3)];
    assert_ne!(
        id,
        flatten_id(&other_path, interior),
        "the descent path changes the id"
    );
    assert_ne!(
        id,
        flatten_id(&path, NodeId::from_u128(8)),
        "the interior id changes the id"
    );

    // A different *instance* of the same composite (distinct leading id) yields
    // a distinct flat id — two copies of one subgraph don't share cache slots.
    let single = [NodeId::from_u128(1)];
    let other_single = [NodeId::from_u128(9)];
    assert_ne!(
        flatten_id(&single, interior),
        flatten_id(&other_single, interior)
    );
}
