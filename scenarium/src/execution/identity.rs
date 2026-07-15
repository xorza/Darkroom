use hashbrown::HashMap;
use serde::{Deserialize, Serialize};

use crate::graph::NodeId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeAddress {
    pub instances: Vec<NodeId>,
    pub node_id: NodeId,
}

impl NodeAddress {
    pub fn root(node_id: NodeId) -> Self {
        Self {
            instances: Vec::new(),
            node_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OutputAddress {
    pub node: NodeAddress,
    pub port_idx: usize,
}

#[derive(Debug, Clone, Default)]
pub struct FlattenMap {
    scopes: Vec<Scope>,
    leaves: HashMap<NodeId, Leaf>,
    flat_nodes: HashMap<NodeAddress, NodeId>,
    representatives: HashMap<NodeId, NodeAddress>,
}

#[derive(Debug, Clone, Copy)]
struct Scope {
    instance: Option<NodeId>,
    parent: u32,
}

#[derive(Debug, Clone)]
struct Leaf {
    scope: u32,
    address: NodeAddress,
}

impl FlattenMap {
    pub(crate) fn reset(&mut self) {
        self.scopes.clear();
        self.leaves.clear();
        self.flat_nodes.clear();
        self.representatives.clear();
        self.scopes.push(Scope {
            instance: None,
            parent: 0,
        });
    }

    pub(crate) fn push_scope(&mut self, instance: NodeId, parent: u32) -> u32 {
        let idx = u32::try_from(self.scopes.len()).expect("flatten scope count exceeds u32");
        self.scopes.push(Scope {
            instance: Some(instance),
            parent,
        });
        idx
    }

    pub(crate) fn set_leaf(&mut self, flat_id: NodeId, scope: u32, interior: NodeId) {
        let address = NodeAddress {
            instances: self.instance_path(scope),
            node_id: interior,
        };
        assert!(
            self.leaves
                .insert(
                    flat_id,
                    Leaf {
                        scope,
                        address: address.clone(),
                    },
                )
                .is_none(),
            "flattened node id collision for {flat_id:?}"
        );
        assert!(
            self.flat_nodes.insert(address.clone(), flat_id).is_none(),
            "duplicate authoring node address {address:?}"
        );
        self.representatives.entry(interior).or_insert(address);
    }

    fn instance_path(&self, mut scope: u32) -> Vec<NodeId> {
        let mut instances = Vec::new();
        loop {
            let entry = self.scopes[scope as usize];
            let Some(instance) = entry.instance else {
                break;
            };
            instances.push(instance);
            scope = entry.parent;
        }
        instances.reverse();
        instances
    }

    pub fn address(&self, flat_id: NodeId) -> Option<&NodeAddress> {
        self.leaves.get(&flat_id).map(|leaf| &leaf.address)
    }

    pub fn flat_node(&self, address: &NodeAddress) -> Option<NodeId> {
        self.flat_nodes.get(address).copied()
    }

    pub fn representative(&self, node_id: &NodeId) -> Option<&NodeAddress> {
        self.representatives.get(node_id)
    }

    pub fn attribution(&self, flat_id: NodeId) -> Attribution<'_> {
        let leaf = self.leaves.get(&flat_id);
        Attribution {
            map: self,
            interior: leaf.map(|leaf| leaf.address.node_id),
            scope: leaf.map(|leaf| leaf.scope),
        }
    }
}

#[derive(Debug)]
pub struct Attribution<'a> {
    map: &'a FlattenMap,
    interior: Option<NodeId>,
    scope: Option<u32>,
}

impl Iterator for Attribution<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        if let Some(id) = self.interior.take() {
            return Some(id);
        }
        let scope = self.map.scopes[self.scope? as usize];
        match scope.instance {
            Some(instance) => {
                self.scope = Some(scope.parent);
                Some(instance)
            }
            None => {
                self.scope = None;
                None
            }
        }
    }
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod test_support {
    use super::*;

    #[derive(Debug)]
    pub struct FlattenMapBuilder {
        map: FlattenMap,
    }

    impl FlattenMapBuilder {
        pub fn new() -> Self {
            let mut map = FlattenMap::default();
            map.reset();
            Self { map }
        }

        pub fn insert_leaf(
            &mut self,
            flat_id: NodeId,
            instances: impl IntoIterator<Item = NodeId>,
            node_id: NodeId,
        ) {
            let mut scope = 0;
            for instance in instances {
                scope = self.map.push_scope(instance, scope);
            }
            self.map.set_leaf(flat_id, scope, node_id);
        }

        pub fn build(self) -> FlattenMap {
            self.map
        }
    }

    impl Default for FlattenMapBuilder {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_exact_nested_addresses_in_both_directions() {
        let outer = NodeId::from_u128(1);
        let inner = NodeId::from_u128(2);
        let interior = NodeId::from_u128(3);
        let flat = NodeId::from_u128(4);
        let mut map = FlattenMap::default();
        map.reset();
        let outer_scope = map.push_scope(outer, 0);
        let inner_scope = map.push_scope(inner, outer_scope);
        map.set_leaf(flat, inner_scope, interior);

        let address = NodeAddress {
            instances: vec![outer, inner],
            node_id: interior,
        };
        assert_eq!(map.address(flat), Some(&address));
        assert_eq!(map.flat_node(&address), Some(flat));
        assert_eq!(
            map.attribution(flat).collect::<Vec<_>>(),
            vec![interior, inner, outer]
        );
    }

    #[test]
    fn keeps_distinct_instances_of_the_same_definition_node() {
        let instance_a = NodeId::from_u128(1);
        let instance_b = NodeId::from_u128(2);
        let interior = NodeId::from_u128(3);
        let flat_a = NodeId::from_u128(4);
        let flat_b = NodeId::from_u128(5);
        let mut map = FlattenMap::default();
        map.reset();
        let scope_a = map.push_scope(instance_a, 0);
        let scope_b = map.push_scope(instance_b, 0);
        map.set_leaf(flat_a, scope_a, interior);
        map.set_leaf(flat_b, scope_b, interior);

        assert_eq!(
            map.flat_node(&NodeAddress {
                instances: vec![instance_a],
                node_id: interior,
            }),
            Some(flat_a)
        );
        assert_eq!(
            map.flat_node(&NodeAddress {
                instances: vec![instance_b],
                node_id: interior,
            }),
            Some(flat_b)
        );
        assert_eq!(map.representative(&interior), map.address(flat_a));
    }
}
