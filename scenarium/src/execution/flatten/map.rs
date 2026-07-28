//! The compact record flattening leaves behind: which authored node and
//! enclosing instances each execution node came from, and which interior
//! nodes back each instance's exposed outputs.
//!
//! Built by the walk in [`super`], read afterwards by
//! [`CompiledGraph`](crate::execution::compile::CompiledGraph). It exists
//! because flattening is lossy — composites and their boundary edges
//! dissolve, so nothing in the finished program can answer either question
//! from its own shape.

use hashbrown::HashMap;
use thiserror::Error;

use crate::execution::identity::ExecutionNodeId;
use crate::graph::NodeId;

#[derive(Debug, Clone, Default)]
pub(crate) struct FlattenMap {
    scopes: Vec<Scope>,
    leaves: HashMap<ExecutionNodeId, Leaf>,
    /// `(graph instance, the execution node behind one of its interface
    /// output ports)`, one entry per wired exposed port per occurrence.
    ///
    /// Recorded because it cannot be recovered afterwards. Flattening
    /// dissolves the `GraphOutput` edges, so in the finished program an
    /// exposed producer read only from inside its own instance is
    /// indistinguishable from interior plumbing — and a "run this
    /// instance" request derived from consumer topology alone would skip
    /// exactly the node the instance exists to produce.
    exposed: Vec<(NodeId, ExecutionNodeId)>,
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
        self.exposed.clear();
        self.scopes.push(Scope {
            instance: None,
            parent: 0,
        });
    }

    /// Note that `producer` backs one of `instance`'s interface output
    /// ports. See [`Self::exposed`].
    pub(crate) fn push_exposed(&mut self, instance: NodeId, producer: ExecutionNodeId) {
        self.exposed.push((instance, producer));
    }

    /// Every `(instance, producer)` pair recorded this build. Handed out
    /// whole rather than filtered per instance, so the caller indexes it
    /// in one pass instead of rescanning for each authored node.
    pub(crate) fn exposed_producers(&self) -> impl Iterator<Item = (NodeId, ExecutionNodeId)> + '_ {
        self.exposed.iter().copied()
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

#[cfg(test)]
pub(crate) mod internals {
    use crate::execution::flatten::map::FlattenMap;
    use crate::execution::identity::ExecutionNodeId;
    use crate::graph::NodeId;

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

    impl Default for FlattenMapBuilder {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::execution::flatten::map::FlattenMap;
    use crate::execution::identity::ExecutionNodeId;
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
