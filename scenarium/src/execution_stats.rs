use hashbrown::HashMap;

use crate::execution::Error;
use crate::graph::{InputPort, NodeId};
use crate::worker::EventRef;

/// A record of how the authoring graph was flattened for execution.
///
/// Execution dissolves subgraphs: an interior node's id is remapped (it's
/// hashed with the chain of composite-instance ids it was reached
/// through), so the flattened `node_id`s in the stat lists don't match
/// the authoring graph for anything inside a subgraph. `FlattenMap` is
/// the reverse: a `scopes` arena (the tree of composite expansions, index
/// 0 = root) plus, per flattened node, the scope it landed in and its
/// authoring id within that scope's (sub)graph.
///
/// Ancestry is shared through the arena's parent links rather than stored
/// as a path per node, so it stays flat and allocation-light. Consume it
/// with [`FlattenMap::attribution`], which yields the authoring node a
/// flattened node maps to plus every enclosing composite instance.
#[derive(Debug, Clone, Default)]
pub struct FlattenMap {
    scopes: Vec<Scope>,
    leaves: HashMap<NodeId, Leaf>,
}

#[derive(Debug, Clone, Copy)]
struct Scope {
    /// Composite-instance node id that opened this scope; `None` for root.
    instance: Option<NodeId>,
    /// Enclosing scope index. Root points at itself (0); the walk stops on
    /// the `None` instance, so the self-loop is never followed.
    parent: u32,
}

#[derive(Debug, Clone, Copy)]
struct Leaf {
    scope: u32,
    interior: NodeId,
}

impl FlattenMap {
    /// Reset to a single root scope (index 0), reusing the buffers — the
    /// flattener calls this at the start of every build.
    pub fn reset(&mut self) {
        self.scopes.clear();
        self.leaves.clear();
        self.scopes.push(Scope {
            instance: None,
            parent: 0,
        });
    }

    /// Open a child scope for composite instance `instance` under
    /// `parent`; returns its index.
    pub fn push_scope(&mut self, instance: NodeId, parent: u32) -> u32 {
        let idx = self.scopes.len() as u32;
        self.scopes.push(Scope {
            instance: Some(instance),
            parent,
        });
        idx
    }

    /// Record that flattened node `flat_id` lives in `scope` as authoring
    /// node `interior`.
    pub fn set_leaf(&mut self, flat_id: NodeId, scope: u32, interior: NodeId) {
        self.leaves.insert(flat_id, Leaf { scope, interior });
    }

    /// The authoring nodes a flattened node's outcome attributes to: its
    /// own interior id, then each enclosing composite instance (innermost
    /// first, root excluded). Empty when `flat_id` is unknown. No
    /// allocation — it walks the scope arena's parent links.
    pub fn attribution(&self, flat_id: NodeId) -> Attribution<'_> {
        let leaf = self.leaves.get(&flat_id);
        Attribution {
            map: self,
            interior: leaf.map(|l| l.interior),
            scope: leaf.map(|l| l.scope),
        }
    }
}

/// Iterator over [`FlattenMap::attribution`]: the interior node id, then
/// the chain of enclosing composite-instance ids.
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

#[derive(Debug, Clone)]
pub struct ExecutedNodeStats {
    pub node_id: NodeId,
    pub elapsed_secs: f64,
}

#[derive(Debug, Clone)]
pub struct NodeError {
    pub node_id: NodeId,
    pub error: Error,
}

#[derive(Debug)]
pub struct ExecutionStats {
    pub elapsed_secs: f64,

    pub executed_nodes: Vec<ExecutedNodeStats>,
    pub missing_inputs: Vec<InputPort>,
    pub cached_nodes: Vec<NodeId>,
    pub triggered_events: Vec<EventRef>,
    pub node_errors: Vec<NodeError>,
    /// How the run's graph was flattened, so a UI can project the
    /// flattened-id stats above back onto the authoring nodes the user
    /// sees (including subgraph interiors + instances).
    pub flatten: FlattenMap,
}
