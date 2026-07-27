//! Strongly typed identities for one flattened compiled graph, plus the compact
//! scope map used to attribute an execution node to authored nodes.
//!
//! Naming convention: `Execution`-prefixed types are the **stable identity
//! space** — they survive installs, cross the host boundary, and may enter
//! digests. `…Id` is a uuid identity; `…Port` pairs one with a port/event
//! index. The install-local **dense index space** (`NodeIdx`, `OutputIdx`,
//! `OutputAddr`) lives in `program/index.rs` under bare names — those types
//! never leave the execution internals, so they need no prefix.

use hashbrown::HashMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;

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
    ///
    /// Minting ids is flatten's business, so this stays inside the crate.
    /// Deriving one answers only for a node flatten emits: a composite
    /// dissolves and has no id of its own, so a host asking "which execution
    /// nodes is this?" would get an id that exists nowhere. Hosts look the
    /// relation up instead — [`CompiledGraph::occurrences`] and its siblings
    /// (`run_targets`, `is_sink`, `is_impure`), which answer for every
    /// authored node.
    ///
    /// [`CompiledGraph::occurrences`]: crate::execution::compile::CompiledGraph::occurrences
    pub(crate) fn from_authoring(path: &[NodeId]) -> Self {
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

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum FlattenMapValidationError {
    #[error("flatten map must have exactly one leaf per execution node")]
    LeafCount,
    #[error("execution node {e_node_id:?} has no flatten-map leaf")]
    MissingLeaf { e_node_id: ExecutionNodeId },
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
    ) -> Result<(), FlattenMapValidationError> {
        let mut seen = 0;
        for e_node_id in e_node_ids {
            if !self.leaves.contains_key(&e_node_id) {
                return Err(FlattenMapValidationError::MissingLeaf { e_node_id });
            }
            seen += 1;
        }
        // Every id had a leaf, so a count mismatch means leaves the program
        // has no node for. Ids are unique (`ExecutionProgram::push` rejects a
        // repeat), so counting them is the same as collecting them.
        if self.leaves.len() != seen {
            return Err(FlattenMapValidationError::LeafCount);
        }
        Ok(())
    }

    /// The authored leaf behind one execution id, then each enclosing graph
    /// instance, innermost first.
    pub(crate) fn attribution(
        &self,
        e_node_id: ExecutionNodeId,
    ) -> Option<impl Iterator<Item = NodeId> + '_> {
        let leaf = self.leaves.get(&e_node_id)?;
        let scope = |scope: u32| self.scopes[scope as usize];
        // The parent chain is walked to the root, which carries no instance
        // and so ends the `map_while` — the only thing that terminates it.
        let enclosing = std::iter::successors(Some(leaf.scope), move |&at| Some(scope(at).parent))
            .map_while(move |at| scope(at).instance);
        Some(std::iter::once(leaf.node_id).chain(enclosing))
    }
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod internals {
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
