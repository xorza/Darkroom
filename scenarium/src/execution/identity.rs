use anyhow::{Result, ensure};
use hashbrown::{HashMap, HashSet};
use serde::{Deserialize, Serialize};

use crate::graph::NodeId;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[repr(transparent)]
pub struct ExecutionNodeId(NodeId);

impl ExecutionNodeId {
    pub(crate) const fn from_node_id(node_id: NodeId) -> Self {
        Self(node_id)
    }

    pub(crate) fn as_uuid(self) -> uuid::Uuid {
        self.0.as_uuid()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExecutionInputPort {
    pub e_node_id: ExecutionNodeId,
    pub port_idx: usize,
}

impl ExecutionInputPort {
    pub(crate) fn new(e_node_id: ExecutionNodeId, port_idx: usize) -> Self {
        Self {
            e_node_id,
            port_idx,
        }
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecutionIdentityError {
    #[error("authoring address {address:?} has no execution identity in this compiled graph")]
    AddressNotFound { address: NodeAddress },
    #[error("execution node {e_node_id:?} has no authoring address in this compiled graph")]
    NodeNotFound { e_node_id: ExecutionNodeId },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OutputAddress {
    pub node: NodeAddress,
    pub port_idx: usize,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FlattenMap {
    scopes: Vec<Scope>,
    leaves: HashMap<ExecutionNodeId, Leaf>,
    execution_nodes: HashMap<NodeAddress, ExecutionNodeId>,
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
        self.execution_nodes.clear();
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

    pub(crate) fn set_leaf(&mut self, e_node_id: ExecutionNodeId, scope: u32, interior: NodeId) {
        let address = NodeAddress {
            instances: self.instance_path(scope),
            node_id: interior,
        };
        let previous_leaf = self.leaves.insert(
            e_node_id,
            Leaf {
                scope,
                address: address.clone(),
            },
        );
        debug_assert!(
            previous_leaf.is_none(),
            "flattened node id collision for {e_node_id:?}"
        );
        let previous_execution = self.execution_nodes.insert(address.clone(), e_node_id);
        debug_assert!(
            previous_execution.is_none(),
            "duplicate authoring node address {address:?}"
        );
    }

    pub(crate) fn validate(
        &self,
        e_node_ids: impl IntoIterator<Item = ExecutionNodeId>,
    ) -> Result<()> {
        let expected: HashSet<_> = e_node_ids.into_iter().collect();
        ensure!(
            self.leaves.len() == expected.len(),
            "flatten map must have exactly one leaf per execution node"
        );
        ensure!(
            self.execution_nodes.len() == self.leaves.len(),
            "flatten map must have one unique authoring address per execution node"
        );

        for e_node_id in expected {
            let Some(leaf) = self.leaves.get(&e_node_id) else {
                anyhow::bail!("execution node {e_node_id:?} has no flatten-map leaf");
            };
            ensure!(
                self.execution_nodes.get(&leaf.address) == Some(&e_node_id),
                "flatten-map address must point back to its execution node"
            );
        }
        for (address, e_node_id) in &self.execution_nodes {
            let Some(leaf) = self.leaves.get(e_node_id) else {
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

    pub(crate) fn address(&self, e_node_id: ExecutionNodeId) -> Option<&NodeAddress> {
        self.leaves.get(&e_node_id).map(|leaf| &leaf.address)
    }

    pub(crate) fn execution_node(&self, address: &NodeAddress) -> Option<ExecutionNodeId> {
        self.execution_nodes.get(address).copied()
    }

    pub(crate) fn attribution(&self, e_node_id: ExecutionNodeId) -> Option<Attribution<'_>> {
        let leaf = self.leaves.get(&e_node_id)?;
        Some(Attribution {
            map: self,
            interior: Some(leaf.address.node_id),
            scope: Some(leaf.scope),
        })
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

#[cfg(any(test, feature = "internals"))]
pub(crate) mod test_support {
    use crate::execution::identity::ExecutionNodeId;
    use crate::graph::NodeId;

    impl ExecutionNodeId {
        pub fn unique() -> Self {
            Self(NodeId::unique())
        }

        pub const fn from_u128(value: u128) -> Self {
            Self(NodeId::from_u128(value))
        }
    }

    #[cfg(test)]
    use crate::execution::identity::{FlattenMap, NodeAddress};

    #[cfg(test)]
    impl FlattenMap {
        pub(crate) fn flat_node(&self, address: &NodeAddress) -> Option<ExecutionNodeId> {
            self.execution_nodes.get(address).copied()
        }
    }

    #[cfg(test)]
    #[derive(Debug)]
    pub(crate) struct FlattenMapBuilder {
        map: FlattenMap,
    }

    #[cfg(test)]
    impl FlattenMapBuilder {
        pub(crate) fn new() -> Self {
            let mut map = FlattenMap::default();
            map.reset();
            Self { map }
        }

        pub(crate) fn insert_leaf(
            &mut self,
            e_node_id: ExecutionNodeId,
            instances: impl IntoIterator<Item = NodeId>,
            node_id: NodeId,
        ) {
            let mut scope = 0;
            for instance in instances {
                scope = self.map.push_scope(instance, scope);
            }
            self.map.set_leaf(e_node_id, scope, node_id);
        }

        pub(crate) fn build(self) -> FlattenMap {
            self.map
        }
    }

    #[cfg(test)]
    impl Default for FlattenMapBuilder {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::execution::identity::{ExecutionNodeId, FlattenMap, NodeAddress};
    use crate::graph::NodeId;

    #[test]
    fn maps_exact_nested_addresses_in_both_directions() {
        let outer = NodeId::from_u128(1);
        let inner = NodeId::from_u128(2);
        let interior = NodeId::from_u128(3);
        let flat = ExecutionNodeId::from_u128(4);
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
            map.attribution(flat).unwrap().collect::<Vec<_>>(),
            vec![interior, inner, outer]
        );
        map.validate([flat]).unwrap();
    }

    #[test]
    fn keeps_distinct_instances_of_the_same_definition_node() {
        let instance_a = NodeId::from_u128(1);
        let instance_b = NodeId::from_u128(2);
        let interior = NodeId::from_u128(3);
        let flat_a = ExecutionNodeId::from_u128(4);
        let flat_b = ExecutionNodeId::from_u128(5);
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
        let flat = ExecutionNodeId::unique();
        let interior = NodeId::unique();
        let mut map = FlattenMap::default();
        map.reset();
        map.set_leaf(flat, 0, interior);

        assert_eq!(
            map.validate([]).unwrap_err().to_string(),
            "flatten map must have exactly one leaf per execution node"
        );
    }

    #[test]
    fn rejects_reverse_identity_mismatch() {
        let flat = ExecutionNodeId::unique();
        let wrong_flat = ExecutionNodeId::unique();
        let interior = NodeId::unique();
        let mut map = FlattenMap::default();
        map.reset();
        map.set_leaf(flat, 0, interior);
        map.execution_nodes
            .insert(NodeAddress::root(interior), wrong_flat);

        assert_eq!(
            map.validate([flat]).unwrap_err().to_string(),
            "flatten-map address must point back to its execution node"
        );
    }
}
