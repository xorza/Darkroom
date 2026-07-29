use super::*;
use crate::execution::identity::ExecutionNodeId;

/// A top-level node (empty descent path) keeps its own id verbatim — this is
/// what lets a func-only graph map to itself and per-node caches survive an
/// edit. (Full flattening of composites is covered by the integration suite.)
#[test]
fn flatten_id_is_identity_at_top_level() {
    let id = NodeId::from_u128(0xABCD);
    assert_eq!(
        NodeId::from(ExecutionNodeId::from_authoring(&[id]).as_uuid()),
        id
    );
}

/// A nested node's id is a deterministic hash of (descent path, interior id):
/// stable across calls, distinct from the bare interior id, and sensitive to
/// both the path and the interior id (so two instances of one composite, or
/// two interior nodes, never collide).
#[test]
fn flatten_id_nested_is_deterministic_and_path_sensitive() {
    let interior = NodeId::from_u128(7);
    let path = [NodeId::from_u128(1), NodeId::from_u128(2), interior];

    let id = ExecutionNodeId::from_authoring(&path);
    assert_eq!(id, ExecutionNodeId::from_authoring(&path), "deterministic");
    assert_ne!(id, ExecutionNodeId::from_authoring(&[interior]));

    let other_path = [NodeId::from_u128(1), NodeId::from_u128(3), interior];
    assert_ne!(
        id,
        ExecutionNodeId::from_authoring(&other_path),
        "the descent path changes the id"
    );
    assert_ne!(
        id,
        ExecutionNodeId::from_authoring(&[
            NodeId::from_u128(1),
            NodeId::from_u128(2),
            NodeId::from_u128(8),
        ]),
        "the interior id changes the id"
    );

    // A different *instance* of the same composite (distinct leading id) yields
    // a distinct flat id — two copies of one graph don't share cache slots.
    assert_ne!(
        ExecutionNodeId::from_authoring(&[NodeId::from_u128(1), interior]),
        ExecutionNodeId::from_authoring(&[NodeId::from_u128(9), interior])
    );
}

#[test]
#[should_panic(expected = "an authoring path must include its leaf node")]
fn rejects_an_empty_authoring_path() {
    ExecutionNodeId::from_authoring(&[]);
}

/// Scopes nest by index: a child names its parent, and the entry graph
/// names no instance — the shape that lets a leaf walk outward without a
/// materialized path, and that makes the walk terminate.
#[test]
fn scopes_nest_under_the_entry_graph() {
    let outer = NodeId::from_u128(1);
    let inner = NodeId::from_u128(2);
    let mut scopes = ScopeTable::open();

    let outer_scope = scopes.push(outer, 0);
    let inner_scope = scopes.push(inner, outer_scope);

    assert_eq!((outer_scope, inner_scope), (1, 2));
    assert_eq!(scopes.len(), 3);
    assert_eq!(scopes.instance(inner_scope), Some(inner));
    assert_eq!(scopes.parent(inner_scope), outer_scope);
    assert_eq!(
        scopes.instance(0),
        None,
        "the entry graph names no instance"
    );
}
