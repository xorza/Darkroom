//! Strongly typed identities for one flattened compiled graph, plus the compact
//! scope map used to attribute an execution node to authored nodes.

use hashbrown::{HashMap, HashSet};
use serde::{Deserialize, Serialize};

use crate::error::{ValidationError, ensure_valid};
use crate::graph::NodeId;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[repr(transparent)]
/// One node in a flattened compiled graph.
pub struct ExecutionNodeId(NodeId);

impl ExecutionNodeId {
    /// Derive an execution identity from a non-empty authoring path, ordered
    /// from the outermost graph instance to the leaf node. A root node uses
    /// `[node_id]`; a nested node uses `[outer_instance, ..., node_id]`.
    pub fn from_authoring(path: &[NodeId]) -> Self {
        let (&node_id, instances) = path
            .split_last()
            .expect("an authoring path must include its leaf node");
        if instances.is_empty() {
            return Self(node_id);
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"scenarium.flatten.v1");
        for instance in instances {
            hasher.update(&instance.as_u128().to_le_bytes());
        }
        hasher.update(&node_id.as_u128().to_le_bytes());
        let digest = hasher.finalize();
        Self(NodeId::from_u128(u128::from_le_bytes(
            digest.as_bytes()[..16].try_into().unwrap(),
        )))
    }

    pub(crate) fn as_uuid(self) -> uuid::Uuid {
        self.0.as_uuid()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// One input port of one flattened execution node.
pub struct ExecutionInputPort {
    pub e_node_id: ExecutionNodeId,
    pub port_idx: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// One output port of one flattened execution node.
pub(crate) struct ExecutionOutputPort {
    pub(crate) e_node_id: ExecutionNodeId,
    pub(crate) port_idx: usize,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
/// One event port of one flattened execution node.
pub struct ExecutionEventPort {
    pub e_node_id: ExecutionNodeId,
    pub event_idx: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
/// A failed lookup from an execution identity to its authoring attribution.
pub enum ExecutionIdentityError {
    #[error("execution node {e_node_id:?} has no authoring attribution in this compiled graph")]
    NodeNotFound { e_node_id: ExecutionNodeId },
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FlattenMap {
    scopes: Vec<Scope>,
    leaves: HashMap<ExecutionNodeId, Leaf>,
}

#[derive(Debug, Clone, Copy)]
struct Scope {
    instance: Option<NodeId>,
    parent: u32,
}

#[derive(Debug, Clone)]
struct Leaf {
    scope: u32,
    node_id: NodeId,
}

impl FlattenMap {
    pub(crate) fn reset(&mut self) {
        self.scopes.clear();
        self.leaves.clear();
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
        let previous_leaf = self.leaves.insert(
            e_node_id,
            Leaf {
                scope,
                node_id: interior,
            },
        );
        debug_assert!(
            previous_leaf.is_none(),
            "flattened node id collision for {e_node_id:?}"
        );
    }

    pub(crate) fn validate(
        &self,
        e_node_ids: impl IntoIterator<Item = ExecutionNodeId>,
    ) -> Result<(), ValidationError> {
        let expected: HashSet<_> = e_node_ids.into_iter().collect();
        ensure_valid!(
            self.leaves.len() == expected.len(),
            "flatten map must have exactly one leaf per execution node"
        );
        for e_node_id in expected {
            if !self.leaves.contains_key(&e_node_id) {
                return Err(ValidationError::new(format!(
                    "execution node {e_node_id:?} has no flatten-map leaf"
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn attribution(&self, e_node_id: ExecutionNodeId) -> Option<Attribution<'_>> {
        let leaf = self.leaves.get(&e_node_id)?;
        Some(Attribution {
            map: self,
            interior: Some(leaf.node_id),
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
    use crate::execution::identity::FlattenMap;

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
    use crate::execution::identity::{ExecutionNodeId, FlattenMap};
    use crate::graph::NodeId;

    #[test]
    fn attributes_nested_execution_nodes_without_materializing_paths() {
        let outer = NodeId::from_u128(1);
        let inner = NodeId::from_u128(2);
        let interior = NodeId::from_u128(3);
        let e_node_id = ExecutionNodeId::from_u128(4);
        let mut map = FlattenMap::default();
        map.reset();
        let outer_scope = map.push_scope(outer, 0);
        let inner_scope = map.push_scope(inner, outer_scope);
        map.set_leaf(e_node_id, inner_scope, interior);

        assert_eq!(
            map.attribution(e_node_id).unwrap().collect::<Vec<_>>(),
            vec![interior, inner, outer]
        );
        map.validate([e_node_id]).unwrap();
    }

    #[test]
    fn keeps_distinct_execution_nodes_for_instances_of_one_definition_node() {
        let instance_a = NodeId::from_u128(1);
        let instance_b = NodeId::from_u128(2);
        let interior = NodeId::from_u128(3);
        let e_node_id_a = ExecutionNodeId::from_u128(4);
        let e_node_id_b = ExecutionNodeId::from_u128(5);
        let mut map = FlattenMap::default();
        map.reset();
        let scope_a = map.push_scope(instance_a, 0);
        let scope_b = map.push_scope(instance_b, 0);
        map.set_leaf(e_node_id_a, scope_a, interior);
        map.set_leaf(e_node_id_b, scope_b, interior);

        assert_eq!(
            map.attribution(e_node_id_a).unwrap().collect::<Vec<_>>(),
            vec![interior, instance_a]
        );
        assert_eq!(
            map.attribution(e_node_id_b).unwrap().collect::<Vec<_>>(),
            vec![interior, instance_b]
        );
        map.validate([e_node_id_a, e_node_id_b]).unwrap();
    }

    #[test]
    fn rejects_execution_node_and_leaf_key_mismatch() {
        let e_node_id = ExecutionNodeId::unique();
        let interior = NodeId::unique();
        let mut map = FlattenMap::default();
        map.reset();
        map.set_leaf(e_node_id, 0, interior);

        assert_eq!(
            map.validate([]).unwrap_err().to_string(),
            "flatten map must have exactly one leaf per execution node"
        );
    }
}
