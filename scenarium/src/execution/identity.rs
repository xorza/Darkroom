use anyhow::{Result, ensure};
use hashbrown::{HashMap, HashSet};
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
pub(crate) struct FlattenMap {
    scopes: Vec<Scope>,
    leaves: HashMap<NodeId, Leaf>,
    flat_nodes: HashMap<NodeAddress, NodeId>,
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
        let previous_leaf = self.leaves.insert(
            flat_id,
            Leaf {
                scope,
                address: address.clone(),
            },
        );
        debug_assert!(
            previous_leaf.is_none(),
            "flattened node id collision for {flat_id:?}"
        );
        let previous_flat = self.flat_nodes.insert(address.clone(), flat_id);
        debug_assert!(
            previous_flat.is_none(),
            "duplicate authoring node address {address:?}"
        );
    }

    pub(crate) fn validate(&self, flat_ids: impl IntoIterator<Item = NodeId>) -> Result<()> {
        let expected: HashSet<_> = flat_ids.into_iter().collect();
        ensure!(
            self.leaves.len() == expected.len(),
            "flatten map must have exactly one leaf per execution node"
        );
        ensure!(
            self.flat_nodes.len() == self.leaves.len(),
            "flatten map forward and reverse identity tables must have equal sizes"
        );

        for flat_id in expected {
            let Some(leaf) = self.leaves.get(&flat_id) else {
                anyhow::bail!("execution node {flat_id:?} has no flatten-map leaf");
            };
            ensure!(
                self.flat_nodes.get(&leaf.address) == Some(&flat_id),
                "flatten-map address must point back to its execution node"
            );
        }
        for (address, flat_id) in &self.flat_nodes {
            let Some(leaf) = self.leaves.get(flat_id) else {
                anyhow::bail!("flatten-map address {address:?} points to no leaf");
            };
            ensure!(
                &leaf.address == address,
                "flatten-map leaf must point back to its authoring address"
            );
        }
        Ok(())
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

    pub(crate) fn address(&self, flat_id: NodeId) -> Option<&NodeAddress> {
        self.leaves.get(&flat_id).map(|leaf| &leaf.address)
    }

    pub(crate) fn flat_node(&self, address: &NodeAddress) -> Option<NodeId> {
        self.flat_nodes.get(address).copied()
    }

    pub(crate) fn attribution(&self, flat_id: NodeId) -> Attribution<'_> {
        let leaf = self.leaves.get(&flat_id);
        Attribution {
            map: self,
            interior: leaf.map(|leaf| leaf.address.node_id),
            scope: leaf.map(|leaf| leaf.scope),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Attribution<'a> {
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

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    #[derive(Debug)]
    pub(crate) struct FlattenMapBuilder {
        map: FlattenMap,
    }

    impl FlattenMapBuilder {
        pub(crate) fn new() -> Self {
            let mut map = FlattenMap::default();
            map.reset();
            Self { map }
        }

        pub(crate) fn insert_leaf(
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

        pub(crate) fn build(self) -> FlattenMap {
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
        map.validate([flat]).unwrap();
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
        map.validate([flat_a, flat_b]).unwrap();
    }

    #[test]
    fn rejects_execution_node_and_leaf_key_mismatch() {
        let flat = NodeId::unique();
        let mut map = FlattenMap::default();
        map.reset();
        map.set_leaf(flat, 0, flat);

        assert_eq!(
            map.validate([]).unwrap_err().to_string(),
            "flatten map must have exactly one leaf per execution node"
        );
    }

    #[test]
    fn rejects_reverse_identity_mismatch() {
        let flat = NodeId::unique();
        let wrong_flat = NodeId::unique();
        let mut map = FlattenMap::default();
        map.reset();
        map.set_leaf(flat, 0, flat);
        map.flat_nodes.insert(NodeAddress::root(flat), wrong_flat);

        assert_eq!(
            map.validate([flat]).unwrap_err().to_string(),
            "flatten-map address must point back to its execution node"
        );
    }
}
