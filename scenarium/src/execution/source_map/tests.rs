use crate::common::column::Column;
use crate::execution::identity::NodeIdx;
use crate::execution::source_map::{Attribution, AttributionValidationError, Leaf, ScopeTable};
use crate::graph::identity::NodeId;

fn column(leaves: impl IntoIterator<Item = Leaf>) -> Column<NodeIdx, Leaf> {
    let mut column = Column::default();
    for leaf in leaves {
        column.push(leaf);
    }
    column
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

/// A leaf two instances deep names its own node, then outwards.
#[test]
fn attributes_nested_execution_nodes_without_materializing_paths() {
    let outer = NodeId::from_u128(1);
    let inner = NodeId::from_u128(2);
    let interior = NodeId::from_u128(3);
    let mut scopes = ScopeTable::open();
    let outer_scope = scopes.push(outer, 0);
    let inner_scope = scopes.push(inner, outer_scope);
    let attribution = Attribution::new(
        scopes,
        column([Leaf {
            scope: inner_scope,
            node_id: interior,
        }]),
    );

    assert_eq!(
        attribution.of(NodeIdx(0)).collect::<Vec<_>>(),
        vec![interior, inner, outer]
    );
    attribution.validate(1).unwrap();
}

/// Two instances of one definition share the authored leaf and differ only
/// in the scope reaching it — the whole reason a leaf carries a scope
/// rather than a materialized path.
#[test]
fn keeps_distinct_execution_nodes_for_instances_of_one_definition_node() {
    let instance_a = NodeId::from_u128(1);
    let instance_b = NodeId::from_u128(2);
    let interior = NodeId::from_u128(3);
    let mut scopes = ScopeTable::open();
    let scope_a = scopes.push(instance_a, 0);
    let scope_b = scopes.push(instance_b, 0);
    let attribution = Attribution::new(
        scopes,
        column([
            Leaf {
                scope: scope_a,
                node_id: interior,
            },
            Leaf {
                scope: scope_b,
                node_id: interior,
            },
        ]),
    );

    assert_eq!(
        attribution.of(NodeIdx(0)).collect::<Vec<_>>(),
        vec![interior, instance_a]
    );
    assert_eq!(
        attribution.of(NodeIdx(1)).collect::<Vec<_>>(),
        vec![interior, instance_b]
    );
    attribution.validate(2).unwrap();
}

#[test]
fn rejects_attribution_that_does_not_span_the_program() {
    let attribution = Attribution::new(
        ScopeTable::open(),
        column([Leaf {
            scope: 0,
            node_id: NodeId::unique(),
        }]),
    );
    assert_eq!(
        attribution.validate(2).unwrap_err(),
        AttributionValidationError::LeafCount {
            len: 1,
            expected: 2
        }
    );
}

/// A leaf naming a scope that isn't there — one of the two index spaces
/// `of` reads unchecked.
#[test]
fn rejects_a_leaf_pointing_outside_the_scope_table() {
    let attribution = Attribution::new(
        ScopeTable::open(),
        column([Leaf {
            scope: 7,
            node_id: NodeId::unique(),
        }]),
    );
    assert_eq!(
        attribution.validate(1).unwrap_err(),
        AttributionValidationError::LeafScope {
            node_idx: NodeIdx(0),
            scope: 7
        }
    );
}

/// The other one: a parent that doesn't precede its scope. `of` would spin
/// on it forever rather than fault, which is why validation looks.
#[test]
fn rejects_a_scope_chain_that_would_not_terminate() {
    let mut scopes = ScopeTable::open();
    // Scope 1, naming itself as its parent.
    let scope = scopes.push(NodeId::unique(), 1);
    let attribution = Attribution::new(
        scopes,
        column([Leaf {
            scope,
            node_id: NodeId::unique(),
        }]),
    );
    assert_eq!(
        attribution.validate(1).unwrap_err(),
        AttributionValidationError::ScopeParent {
            scope: 1,
            parent: 1
        }
    );
}
